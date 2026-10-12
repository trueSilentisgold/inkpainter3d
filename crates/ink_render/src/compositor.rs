//! The layer stack on the GPU: combines a texture set's layers into the channel textures the
//! viewport samples.
//!
//! The texture is composited in chunks of [`CHUNK`] texels. For each chunk, the masks of
//! the stack are evaluated first; then, one channel after the other, every layer is blended
//! onto an accumulator the size of the chunk (composite.wgsl). Folders composite their
//! children on accumulators of their own, unless they pass through. A resolve pass turns
//! the channels into the output textures, deriving normals from the height, and the mips
//! follow.
//!
//! What changed decides what is composited: painting updates only the painted texels (plus
//! a margin for the padding around the UV islands); any other change to the stack
//! recomposites the whole texture, a few chunks per frame so the UI stays responsive.

use crate::paint::{PaintCaches, PoolKind, begin_pass};
use crate::region::{Region, TexelRect};
use crate::viewport::{CHANNEL_FORMATS, GpuMesh, srgb_to_linear};
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use ink_core::export::{ExportBlock, ExportTexel};
use ink_core::{
    BlendMode, Channel, ChannelKind, Content, EffectKind, Layer, LayerStack, Mask, Model,
    Projection, ProjectionMode, Texels, TextureImage,
};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Side of the square chunks the texture is composited in.
const CHUNK: u32 = 512;
/// Extra texels around a chunk, for the neighbours the height-to-normal filter reads.
const APRON: u32 = 1;
const TEMP: u32 = CHUNK + 2 * APRON;
const ACC_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
const SEED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
/// What the export reads back: four of these per texel (composite_resolve.wgsl, fs_export).
const EXPORT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const EXPORT_TARGETS: usize = 4;
const GEOMETRY_FORMATS: [wgpu::TextureFormat; 3] = [
    wgpu::TextureFormat::Rgba32Float,
    wgpu::TextureFormat::Rgba16Float,
    wgpu::TextureFormat::Rgba16Float,
];
/// Uniform buffer offset alignment of the per-pass parameters.
const PASS_STRIDE: u64 = 256;
/// Texels around painted ones that are composited too while painting, for the padding past
/// the UV islands. The rest of the padding follows when the stroke ends.
const PADDING_MARGIN: u32 = 8;
/// How far the padding is brought up to date right when a stroke ends. The chunks within
/// one chunk of the stroke follow a few per frame; farther texels only matter for the
/// smallest mips and catch up with the next full composite.
const STROKE_CATCH_UP: u32 = 64;
/// Chunks of a full composite done per frame.
const CHUNKS_PER_FRAME: usize = 4;

/// What shows where no layer put anything: Painter's shader defaults, linear.
fn background(channel: Channel) -> [f32; 4] {
    match channel {
        Channel::BaseColor => [0.5, 0.5, 0.5, 0.0],
        Channel::Height => [0.0; 4],
        Channel::Roughness => [0.3, 0.0, 0.0, 0.0],
        Channel::Metallic => [0.0; 4],
        Channel::Normal => [0.5, 0.5, 1.0, 0.0],
        Channel::Emissive => [0.0; 4],
        Channel::Opacity => [1.0, 0.0, 0.0, 0.0],
    }
}

/// An accumulator with nothing in it.
fn empty(channel: Channel) -> wgpu::Color {
    match channel.kind() {
        ChannelKind::Normal => wgpu::Color {
            r: 0.5,
            g: 0.5,
            b: 1.0,
            a: 0.0,
        },
        _ => wgpu::Color::TRANSPARENT,
    }
}

/// Channels stored in 8 bits: blending clamps them to 0..1, as in Painter.
fn clamped(channel: Channel) -> bool {
    channel != Channel::Height && channel != Channel::Normal
}

fn kind_code(kind: ChannelKind) -> u32 {
    match kind {
        ChannelKind::Color => 0,
        ChannelKind::Scalar => 1,
        ChannelKind::Normal => 2,
    }
}

const FLAG_MASK: u32 = 1;
const FLAG_IMAGE: u32 = 2;
const FLAG_CLAMP: u32 = 4;
const FLAG_TRIPLANAR: u32 = 8;
const FLAG_HEIGHT_IMAGE: u32 = 16;
const FLAG_NORMAL_IMAGE: u32 = 32;
const FLAG_PAINT: u32 = 64;
const FLAG_DIRECTX: u32 = 128;

/// Must match `Pass` in composite.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct PassUniforms {
    info: [u32; 4],
    area: [i32; 4],
    value: [f32; 4],
    params: [f32; 4],
    transform: [f32; 4],
    offset: [f32; 4],
    levels: [f32; 4],
    levels2: [f32; 4],
}

/// Must match `Resolve` in composite_resolve.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct ResolveUniforms {
    area: [i32; 4],
    params: [f32; 4],
    base_color: [f32; 4],
    surface: [f32; 4],
}

/// Must match `Geometry` in composite_geometry.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct GeometryUniforms {
    area: [i32; 4],
    bounds: [f32; 4],
}

/// A texture the compositor works with, by role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tex {
    /// Accumulator of a nesting depth, one of two for ping-pong.
    Acc(usize, usize),
    /// What lay below a passthrough folder, per nesting level of passthrough folders.
    Save(usize),
    /// Mask evaluation, one of two for ping-pong.
    MaskTmp(usize),
    /// A finished mask, per masked layer.
    MaskCache(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pipe {
    LayerPaint,
    LayerFill,
    Folder,
    Passthrough,
    MaskPaint,
    MaskEffect,
    MaskLevels,
}

/// One step of compositing a chunk.
enum Step {
    Clear(Tex, wgpu::Color),
    Copy(Tex, Tex),
    Pass(Box<PassStep>),
    /// The finished channel, into the results.
    Result(Tex, Channel),
}

struct PassStep {
    pipe: Pipe,
    uniforms: PassUniforms,
    below: Tex,
    other: Option<Tex>,
    mask: Option<Tex>,
    /// Paint layer id and the pool of the channel read.
    paint: Option<(u64, PoolKind)>,
    image: Option<(Arc<TextureImage>, bool)>,
    target: Tex,
}

/// The steps for one composite of a texture set, the same for every chunk but for the
/// chunk's position.
#[derive(Default)]
struct Program {
    steps: Vec<Step>,
    depth: usize,
    saves: usize,
    masks: usize,
    triplanar: bool,
}

/// Builds a [`Program`] from a layer stack.
struct Builder<'a> {
    paint: &'a PaintCaches,
    resolution: u32,
    program: Program,
    /// Current accumulator of each depth.
    current: Vec<usize>,
    mask_slots: HashMap<u64, usize>,
    save_level: usize,
}

impl Builder<'_> {
    fn build(mut self, stack: &LayerStack) -> Program {
        self.collect_masks(&stack.layers);
        for channel in Channel::ALL {
            self.current = vec![0];
            self.program
                .steps
                .push(Step::Clear(Tex::Acc(0, 0), empty(channel)));
            self.walk(&stack.layers, 0, channel);
            self.program
                .steps
                .push(Step::Result(Tex::Acc(0, self.current[0]), channel));
        }
        self.program
    }

    /// Evaluates the masks of visible layers into their cache slots.
    fn collect_masks(&mut self, layers: &[Layer]) {
        for layer in layers.iter().filter(|l| l.visible) {
            if let Some(mask) = layer.mask.as_ref().filter(|m| m.enabled) {
                let slot = self.program.masks;
                self.program.masks += 1;
                self.mask_slots.insert(layer.id, slot);
                self.mask_steps(mask, slot);
            }
            if let Some(children) = layer.children() {
                self.collect_masks(children);
            }
        }
    }

    fn mask_steps(&mut self, mask: &Mask, slot: usize) {
        let gray = |v: f32| wgpu::Color {
            r: v as f64,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let painted = self.has_paint(mask.paint.id, 0);
        // Disable leaves an effect out, as for layers.
        let effects: Vec<_> = mask
            .effects
            .iter()
            .filter(|e| e.visible && e.blend != BlendMode::Disable)
            .collect();
        if !painted && effects.is_empty() {
            self.program
                .steps
                .push(Step::Clear(Tex::MaskCache(slot), gray(mask.background)));
            return;
        }
        let mut cur = 0;
        self.program
            .steps
            .push(Step::Clear(Tex::MaskTmp(0), gray(mask.background)));
        let mut push = |builder: &mut Self, pipe, uniforms, paint, image| {
            builder.program.steps.push(Step::Pass(Box::new(PassStep {
                pipe,
                uniforms,
                below: Tex::MaskTmp(cur),
                other: None,
                mask: None,
                paint,
                image,
                target: Tex::MaskTmp(1 - cur),
            })));
            cur = 1 - cur;
        };
        if painted {
            let uniforms = self.paint_uniforms(mask.paint.id, 0, ChannelKind::Scalar);
            push(
                self,
                Pipe::MaskPaint,
                uniforms,
                Some((mask.paint.id, PoolKind::Rg)),
                None,
            );
        }
        for effect in effects {
            let mut uniforms = PassUniforms {
                info: [effect.blend.code(), 1, FLAG_CLAMP, 0],
                params: [effect.opacity, 0.0, 0.0, 0.0],
                ..Default::default()
            };
            match &effect.kind {
                EffectKind::Paint(paint) => {
                    if !self.has_paint(paint.id, 0) && effect.blend != BlendMode::Replace {
                        continue;
                    }
                    let mut paint_uniforms = self.paint_uniforms(paint.id, 0, ChannelKind::Scalar);
                    paint_uniforms.info[0] = effect.blend.code();
                    paint_uniforms.info[2] |= FLAG_CLAMP;
                    paint_uniforms.params[0] = effect.opacity;
                    push(
                        self,
                        Pipe::MaskEffect,
                        paint_uniforms,
                        Some((paint.id, PoolKind::Rg)),
                        None,
                    );
                }
                EffectKind::Fill(fill) => {
                    uniforms.value = [fill.value, 0.0, 0.0, 0.0];
                    let image = fill.image.clone();
                    if let Some(image) = &image {
                        self.projection(&mut uniforms, &fill.projection, image);
                    }
                    push(
                        self,
                        Pipe::MaskEffect,
                        uniforms,
                        None,
                        image.map(|i| (i, false)),
                    );
                }
                EffectKind::Levels(levels) => {
                    uniforms.levels = [
                        levels.input[0],
                        levels.input[1],
                        levels.gamma,
                        levels.output[0],
                    ];
                    uniforms.levels2 = [levels.output[1], 0.0, 0.0, 0.0];
                    push(self, Pipe::MaskLevels, uniforms, None, None);
                }
            }
        }
        self.program
            .steps
            .push(Step::Copy(Tex::MaskTmp(cur), Tex::MaskCache(slot)));
    }

    fn has_paint(&self, paint: u64, channel: usize) -> bool {
        self.paint.map(paint).is_some_and(|m| m.has_paint(channel))
    }

    /// Uniforms that read a paint layer's channel.
    fn paint_uniforms(&self, paint: u64, channel: usize, kind: ChannelKind) -> PassUniforms {
        let (side, offset) = self.paint.map(paint).map_or((1, 0), |m| {
            (m.tiles_side, channel as u32 * m.tiles_side * m.tiles_side)
        });
        PassUniforms {
            info: [0, kind_code(kind), FLAG_PAINT, side],
            area: [0, 0, self.resolution as i32, offset as i32],
            params: [1.0, 0.0, 0.0, 0.0],
            ..Default::default()
        }
    }

    /// Fills in how an image is projected.
    fn projection(
        &mut self,
        uniforms: &mut PassUniforms,
        projection: &Projection,
        image: &TextureImage,
    ) {
        uniforms.info[2] |= FLAG_IMAGE;
        uniforms.params[1] = image.width.max(image.height) as f32;
        uniforms.params[2] = projection.hardness;
        uniforms.transform = [
            projection.scale[0],
            projection.scale[1],
            projection.rotation.to_radians(),
            0.0,
        ];
        uniforms.offset = [projection.offset[0], projection.offset[1], 0.0, 0.0];
        if projection.mode == ProjectionMode::TriPlanar {
            // Painter has no UV offset for tri-planar projections.
            uniforms.info[2] |= FLAG_TRIPLANAR;
            uniforms.offset = [0.0; 4];
            self.program.triplanar = true;
        }
    }

    fn acc(&mut self, depth: usize) -> Tex {
        Tex::Acc(depth, self.current[depth])
    }

    /// A pass that reads the accumulator of `depth` and writes the other one.
    fn push_acc(&mut self, depth: usize, mut step: PassStep) {
        step.below = self.acc(depth);
        self.current[depth] = 1 - self.current[depth];
        step.target = self.acc(depth);
        self.program.steps.push(Step::Pass(Box::new(step)));
    }

    fn walk(&mut self, layers: &[Layer], depth: usize, channel: Channel) {
        self.program.depth = self.program.depth.max(depth + 1);
        let kind = channel.kind();
        for layer in layers.iter().filter(|l| l.visible) {
            let blend = layer.blending.get(channel);
            if blend.mode == BlendMode::Disable {
                continue;
            }
            let mask = self.mask_slots.get(&layer.id).map(|&k| Tex::MaskCache(k));
            let mut flags = if mask.is_some() { FLAG_MASK } else { 0 };
            if clamped(channel) {
                flags |= FLAG_CLAMP;
            }
            let base = PassUniforms {
                info: [blend.mode.code(), kind_code(kind), flags, 0],
                area: [0, 0, self.resolution as i32, 0],
                params: [blend.opacity, 0.0, 0.0, 0.0],
                ..Default::default()
            };
            let step = |pipe, uniforms| PassStep {
                pipe,
                uniforms,
                below: Tex::Acc(0, 0),
                other: None,
                mask,
                paint: None,
                image: None,
                target: Tex::Acc(0, 0),
            };
            match &layer.content {
                Content::Paint(paint) => {
                    let c = channel.index();
                    if !self.has_paint(paint.id, c) && blend.mode != BlendMode::Replace {
                        continue;
                    }
                    let mut uniforms = self.paint_uniforms(paint.id, c, kind);
                    uniforms.info[0] = base.info[0];
                    uniforms.info[2] |= flags;
                    uniforms.params = base.params;
                    let pool = crate::paint::pool_kind(false, c);
                    let mut s = step(Pipe::LayerPaint, uniforms);
                    s.paint = Some((paint.id, pool));
                    self.push_acc(depth, s);
                }
                Content::Fill(fill) => {
                    if !fill.channels.contains(channel) {
                        continue;
                    }
                    let mut uniforms = base;
                    let v = fill.values.get(channel);
                    uniforms.value = match kind {
                        ChannelKind::Color => {
                            let [r, g, b] = v.map(srgb_to_linear);
                            [r, g, b, 1.0]
                        }
                        _ => [v[0], v[1], v[2], 1.0],
                    };
                    let image = fill.image(channel).cloned();
                    if let Some(image) = &image {
                        self.projection(&mut uniforms, &fill.projection, image);
                        match channel {
                            Channel::Height => uniforms.info[2] |= FLAG_HEIGHT_IMAGE,
                            Channel::Normal => {
                                uniforms.info[2] |= FLAG_NORMAL_IMAGE;
                                if fill.normal_directx {
                                    uniforms.info[2] |= FLAG_DIRECTX;
                                }
                            }
                            _ => {}
                        }
                    }
                    let mut s = step(Pipe::LayerFill, uniforms);
                    s.image = image.map(|i| (i, kind == ChannelKind::Color));
                    self.push_acc(depth, s);
                }
                Content::Folder(folder) => {
                    if blend.mode == BlendMode::Passthrough {
                        let lerp = mask.is_some() || blend.opacity < 1.0;
                        let level = self.save_level;
                        if lerp {
                            self.save_level += 1;
                            self.program.saves = self.program.saves.max(self.save_level);
                            let from = self.acc(depth);
                            self.program.steps.push(Step::Copy(from, Tex::Save(level)));
                        }
                        self.walk(&folder.layers, depth, channel);
                        if lerp {
                            let mut s = step(Pipe::Passthrough, base);
                            s.other = Some(Tex::Save(level));
                            self.push_acc(depth, s);
                            self.save_level -= 1;
                        }
                    } else {
                        if self.current.len() <= depth + 1 {
                            self.current.push(0);
                        }
                        self.current[depth + 1] = 0;
                        self.program
                            .steps
                            .push(Step::Clear(Tex::Acc(depth + 1, 0), empty(channel)));
                        self.walk(&folder.layers, depth + 1, channel);
                        let mut s = step(Pipe::Folder, base);
                        s.other = Some(self.acc(depth + 1));
                        self.push_acc(depth, s);
                    }
                }
            }
        }
    }
}

/// Compositing state of one texture set.
pub(crate) struct GpuComposite {
    resolution: u32,
    /// For every texel, the nearest texel covered by the set's triangles.
    seeds: wgpu::TextureView,
    /// For every covered texel, the index of its triangle; made when a tri-planar
    /// projection needs it.
    triangles: Option<wgpu::TextureView>,
    /// Hash of everything in the stack that shapes the result, except strokes.
    signature: Option<u64>,
    /// Chunks still to composite, row by row.
    pending: Vec<bool>,
    cursor: usize,
    /// What the stroke in progress has painted, for the padding catch-up when it ends.
    stroke_region: Region,
}

impl GpuComposite {
    fn chunks_side(&self) -> u32 {
        self.resolution.div_ceil(CHUNK)
    }

    fn chunk_rect(&self, index: usize) -> TexelRect {
        let side = self.chunks_side();
        let (x, y) = (index as u32 % side, index as u32 / side);
        TexelRect {
            min: [x * CHUNK, y * CHUNK],
            max: [
                ((x + 1) * CHUNK).min(self.resolution),
                ((y + 1) * CHUNK).min(self.resolution),
            ],
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.iter().any(|&p| p)
    }

    /// Queues the chunks that overlap `rect` for compositing.
    fn mark_pending(&mut self, rect: TexelRect) {
        if rect.is_empty() {
            return;
        }
        let side = self.chunks_side();
        for y in rect.min[1] / CHUNK..rect.max[1].div_ceil(CHUNK).min(side) {
            for x in rect.min[0] / CHUNK..rect.max[0].div_ceil(CHUNK).min(side) {
                self.pending[(y * side + x) as usize] = true;
            }
        }
    }
}

/// Textures the size of a chunk.
struct Temps {
    acc: Vec<[wgpu::Texture; 2]>,
    saves: Vec<wgpu::Texture>,
    mask_tmp: [wgpu::Texture; 2],
    masks: Vec<wgpu::Texture>,
    /// The finished channels, one array layer each.
    results: wgpu::Texture,
    results_view: wgpu::TextureView,
    geometry: [wgpu::Texture; 3],
}

/// Where [`Compositor::run`] puts the composited chunks.
enum Target<'a> {
    /// The texture set's channel textures.
    Channels(&'a [wgpu::Texture]),
    /// Read back for the export.
    Export(&'a Readback, &'a mut dyn FnMut(ExportBlock)),
}

/// Targets a chunk is exported to and the buffer it is read back through.
struct Readback {
    targets: Vec<wgpu::Texture>,
    views: Vec<wgpu::TextureView>,
    buffer: wgpu::Buffer,
    /// Why a chunk could not be read back.
    error: std::cell::RefCell<Option<String>>,
}

impl Readback {
    /// Bytes per row of a target in the buffer, aligned for the copy.
    const ROW: u32 = (CHUNK * 8).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    const PLANE: u64 = Self::ROW as u64 * CHUNK as u64;

    fn new(device: &wgpu::Device) -> Self {
        let targets: Vec<wgpu::Texture> = (0..EXPORT_TARGETS)
            .map(|_| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("export target"),
                    size: wgpu::Extent3d {
                        width: CHUNK,
                        height: CHUNK,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: EXPORT_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            })
            .collect();
        Self {
            views: targets
                .iter()
                .map(|t| t.create_view(&Default::default()))
                .collect(),
            targets,
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("export readback"),
                size: Self::PLANE * EXPORT_TARGETS as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            error: Default::default(),
        }
    }

    /// Resolves a `w` × `h` chunk into the targets and copies it to the buffer.
    fn resolve(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        group: &wgpu::BindGroup,
        w: u32,
        h: u32,
    ) {
        {
            let attachments: Vec<_> = self
                .views
                .iter()
                .map(|view| {
                    Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })
                })
                .collect();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite export"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_scissor_rect(0, 0, w, h);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        for (i, target) in self.targets.iter().enumerate() {
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: i as u64 * Self::PLANE,
                        bytes_per_row: Some(Self::ROW),
                        rows_per_image: Some(CHUNK),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Waits for the copy and reads the texels of `rect`.
    fn read(&self, device: &wgpu::Device, rect: TexelRect) -> Result<ExportBlock, String> {
        let width = rect.max[0] - rect.min[0];
        let height = rect.max[1] - rect.min[1];
        let slice = self.buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| e.to_string())?;
        let mut texels = Vec::with_capacity((width * height) as usize);
        {
            let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
            let value = |plane: usize, x: u32, y: u32| -> [f32; 4] {
                let offset = plane * Self::PLANE as usize + (y * Self::ROW + x * 8) as usize;
                std::array::from_fn(|c| {
                    let bytes = [data[offset + 2 * c], data[offset + 2 * c + 1]];
                    half::f16::from_le_bytes(bytes).to_f32()
                })
            };
            for y in 0..height {
                for x in 0..width {
                    texels.push(export_texel([
                        value(0, x, y),
                        value(1, x, y),
                        value(2, x, y),
                        value(3, x, y),
                    ]));
                }
            }
        }
        self.buffer.unmap();
        Ok(ExportBlock {
            x: rect.min[0],
            y: rect.min[1],
            width,
            height,
            texels,
        })
    }
}

/// A texel from the export targets; must match `ExportOutputs` in composite_resolve.wgsl.
fn export_texel([base, normal, surface, emissive]: [[f32; 4]; EXPORT_TARGETS]) -> ExportTexel {
    ExportTexel {
        base_color: [base[0], base[1], base[2]],
        opacity: base[3],
        height: normal[3],
        roughness: surface[0],
        metallic: surface[1],
        normal: [normal[0], normal[1], normal[2]],
        emissive: [emissive[0], emissive[1], emissive[2]],
        island_distance: surface[2],
    }
}

pub(crate) struct Compositor {
    static_layout: wgpu::BindGroupLayout,
    pass_layout: wgpu::BindGroupLayout,
    resolve_layout: wgpu::BindGroupLayout,
    geometry_layout: wgpu::BindGroupLayout,
    triangles_layout: wgpu::BindGroupLayout,
    jfa_layout: wgpu::BindGroupLayout,
    mip_layout: wgpu::BindGroupLayout,
    pipes: HashMap<PipeKey, wgpu::RenderPipeline>,
    resolve: wgpu::RenderPipeline,
    export: wgpu::RenderPipeline,
    geometry: wgpu::RenderPipeline,
    triangles: wgpu::RenderPipeline,
    seed_init: wgpu::RenderPipeline,
    jfa: wgpu::RenderPipeline,
    mip_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    image_sampler: wgpu::Sampler,
    linear_sampler: wgpu::Sampler,
    pass_buffer: wgpu::Buffer,
    resolve_buffer: wgpu::Buffer,
    geometry_buffer: wgpu::Buffer,
    temps: Option<Temps>,
    /// Uploaded fill images by image id and whether they hold colors (sRGB).
    images: HashMap<(u64, bool), wgpu::TextureView>,
    /// Per texture set, the images its last composite used.
    set_images: Vec<HashSet<(u64, bool)>>,
    dummy_acc: wgpu::TextureView,
    dummy_mask: wgpu::TextureView,
    dummy_tiles: wgpu::TextureView,
    dummy_slots: wgpu::Buffer,
    dummy_image: wgpu::TextureView,
    dummy_geometry: [wgpu::TextureView; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PipeKey {
    Acc(PipeAcc),
    Mask(PipeMask),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PipeAcc {
    LayerPaint,
    LayerFill,
    Folder,
    Passthrough,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PipeMask {
    Paint,
    Effect,
    Levels,
}

impl Pipe {
    fn key(self) -> PipeKey {
        match self {
            Self::LayerPaint => PipeKey::Acc(PipeAcc::LayerPaint),
            Self::LayerFill => PipeKey::Acc(PipeAcc::LayerFill),
            Self::Folder => PipeKey::Acc(PipeAcc::Folder),
            Self::Passthrough => PipeKey::Acc(PipeAcc::Passthrough),
            Self::MaskPaint => PipeKey::Mask(PipeMask::Paint),
            Self::MaskEffect => PipeKey::Mask(PipeMask::Effect),
            Self::MaskLevels => PipeKey::Mask(PipeMask::Levels),
        }
    }
}

impl Compositor {
    pub fn new(device: &wgpu::Device, vertex_layout: &wgpu::VertexBufferLayout) -> Self {
        let module = |label: &str, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let composite = module("composite", include_str!("composite.wgsl"));
        let resolve_shader = module("composite resolve", include_str!("composite_resolve.wgsl"));
        let geometry_shader = module(
            "composite geometry",
            include_str!("composite_geometry.wgsl"),
        );
        let seeds_shader = module("paint seeds", include_str!("paint_seeds.wgsl"));
        let mip_shader = module("mipmap", include_str!("mipmap.wgsl"));

        let fragment = wgpu::ShaderStages::FRAGMENT;
        let texture =
            |binding, sample_type, view_dimension, visibility| wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty: wgpu::BindingType::Texture {
                    sample_type,
                    view_dimension,
                    multisampled: false,
                },
                count: None,
            };
        let d2 = wgpu::TextureViewDimension::D2;
        let float = wgpu::TextureSampleType::Float { filterable: false };
        let filterable = wgpu::TextureSampleType::Float { filterable: true };
        let uint = wgpu::TextureSampleType::Uint;
        let buffer = |binding, ty, dynamic, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
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
        let storage = wgpu::BufferBindingType::Storage { read_only: true };
        let static_layout = layout(
            "composite static",
            &[
                texture(0, uint, d2, fragment),
                sampler(1),
                texture(2, float, d2, fragment),
                texture(3, float, d2, fragment),
                texture(4, float, d2, fragment),
            ],
        );
        let pass_layout = layout(
            "composite pass",
            &[
                buffer(0, uniform, true, fragment),
                texture(1, float, d2, fragment),
                texture(2, float, d2, fragment),
                texture(3, float, d2, fragment),
                texture(4, float, wgpu::TextureViewDimension::D2Array, fragment),
                buffer(5, storage, false, fragment),
                texture(6, filterable, d2, fragment),
            ],
        );
        let resolve_layout = layout(
            "composite resolve",
            &[
                buffer(0, uniform, false, fragment),
                texture(1, float, wgpu::TextureViewDimension::D2Array, fragment),
                texture(2, uint, d2, fragment),
            ],
        );
        let geometry_layout = layout(
            "composite geometry",
            &[
                buffer(0, uniform, false, fragment),
                texture(1, uint, d2, fragment),
                texture(2, uint, d2, fragment),
                buffer(3, storage, false, fragment),
                buffer(4, storage, false, fragment),
            ],
        );
        let triangles_layout = layout(
            "triangle ids",
            &[
                buffer(5, storage, false, wgpu::ShaderStages::VERTEX),
                buffer(6, storage, false, wgpu::ShaderStages::VERTEX),
            ],
        );
        let jfa_layout = layout("jfa", &[texture(0, uint, d2, fragment)]);
        let mip_layout = layout(
            "mipmap",
            &[texture(0, filterable, d2, fragment), sampler(1)],
        );

        let pipeline = |label: &str,
                        layouts: &[&wgpu::BindGroupLayout],
                        shader: &wgpu::ShaderModule,
                        vs: &str,
                        fs: &str,
                        buffers: &[Option<wgpu::VertexBufferLayout>],
                        formats: &[wgpu::TextureFormat]| {
            let groups: Vec<Option<&wgpu::BindGroupLayout>> =
                layouts.iter().map(|&l| Some(l)).collect();
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });
            let targets: Vec<Option<wgpu::ColorTargetState>> = formats
                .iter()
                .map(|&format| {
                    Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })
                })
                .collect();
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let groups = [&static_layout, &pass_layout];
        let mut pipes = HashMap::new();
        for (key, fs) in [
            (PipeAcc::LayerPaint, "fs_layer_paint"),
            (PipeAcc::LayerFill, "fs_layer_fill"),
            (PipeAcc::Folder, "fs_folder"),
            (PipeAcc::Passthrough, "fs_passthrough"),
        ] {
            let p = pipeline(
                "composite",
                &groups,
                &composite,
                "vs_fullscreen",
                fs,
                &[],
                &[ACC_FORMAT],
            );
            pipes.insert(PipeKey::Acc(key), p);
        }
        for (key, fs) in [
            (PipeMask::Paint, "fs_mask_paint"),
            (PipeMask::Effect, "fs_mask_effect"),
            (PipeMask::Levels, "fs_mask_levels"),
        ] {
            let p = pipeline(
                "composite mask",
                &groups,
                &composite,
                "vs_fullscreen",
                fs,
                &[],
                &[MASK_FORMAT],
            );
            pipes.insert(PipeKey::Mask(key), p);
        }
        let resolve = pipeline(
            "composite resolve",
            &[&resolve_layout],
            &resolve_shader,
            "vs_fullscreen",
            "fs_resolve",
            &[],
            &CHANNEL_FORMATS,
        );
        let export = pipeline(
            "composite export",
            &[&resolve_layout],
            &resolve_shader,
            "vs_fullscreen",
            "fs_export",
            &[],
            &[EXPORT_FORMAT; EXPORT_TARGETS],
        );
        let geometry = pipeline(
            "composite geometry",
            &[&geometry_layout],
            &geometry_shader,
            "vs_fullscreen",
            "fs_geometry",
            &[],
            &GEOMETRY_FORMATS,
        );
        let triangles = pipeline(
            "triangle ids",
            &[&triangles_layout],
            &geometry_shader,
            "vs_triangles",
            "fs_triangles",
            &[],
            &[SEED_FORMAT],
        );
        let mesh_buffers = [Some(vertex_layout.clone())];
        let seed_init = pipeline(
            "paint seed init",
            &[],
            &seeds_shader,
            "vs_uv",
            "fs_init",
            &mesh_buffers,
            &[SEED_FORMAT],
        );
        let jfa = pipeline(
            "paint jump flood",
            &[&jfa_layout],
            &seeds_shader,
            "vs_jfa",
            "fs_jfa",
            &[],
            &[SEED_FORMAT],
        );
        let mip_pipelines = CHANNEL_FORMATS
            .into_iter()
            .map(|format| {
                let p = pipeline(
                    "mipmap",
                    &[&mip_layout],
                    &mip_shader,
                    "vs_fullscreen",
                    "fs_downsample",
                    &[],
                    &[format],
                );
                (format, p)
            })
            .collect();

        let image_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fill images"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mipmap sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let small = |label: &str, format, layers: u32, dimension| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: layers,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(dimension),
                    ..Default::default()
                })
        };
        Self {
            pipes,
            resolve,
            export,
            geometry,
            triangles,
            seed_init,
            jfa,
            mip_pipelines,
            image_sampler,
            linear_sampler,
            pass_buffer: buffer(
                "composite passes",
                PASS_STRIDE * 64,
                wgpu::BufferUsages::UNIFORM,
            ),
            resolve_buffer: buffer(
                "composite resolve",
                size_of::<ResolveUniforms>() as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
            geometry_buffer: buffer(
                "composite geometry",
                size_of::<GeometryUniforms>() as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
            temps: None,
            images: HashMap::new(),
            set_images: Vec::new(),
            dummy_acc: small("dummy", ACC_FORMAT, 1, d2),
            dummy_mask: small("dummy mask", MASK_FORMAT, 1, d2),
            dummy_tiles: small(
                "dummy tiles",
                ACC_FORMAT,
                1,
                wgpu::TextureViewDimension::D2Array,
            ),
            dummy_slots: buffer("dummy slots", 16, wgpu::BufferUsages::STORAGE),
            dummy_image: small("dummy image", wgpu::TextureFormat::Rgba8Unorm, 1, d2),
            dummy_geometry: GEOMETRY_FORMATS.map(|f| small("dummy geometry", f, 1, d2)),
            static_layout,
            pass_layout,
            resolve_layout,
            geometry_layout,
            triangles_layout,
            jfa_layout,
            mip_layout,
        }
    }

    /// Compositing state for a texture set at this resolution; the outputs are filled with
    /// the background until the first composite.
    pub fn create_set(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        set: usize,
        resolution: u32,
    ) -> GpuComposite {
        let resolution = resolution.max(1);
        let seeds = self.compute_seeds(device, queue, model, mesh, set, resolution);
        let mut state = GpuComposite {
            resolution,
            seeds,
            triangles: None,
            signature: None,
            pending: Vec::new(),
            cursor: 0,
            stroke_region: Region::default(),
        };
        let side = state.chunks_side() as usize;
        state.pending = vec![false; side * side];
        state
    }

    /// Brings a texture set's outputs up to date with its stack. `changed` are the texels
    /// whose paint changed. While `stroke_in_progress`, only those are composited (with a
    /// margin) and full composites wait; when the stroke ends, the padding around it
    /// catches up.
    #[allow(clippy::too_many_arguments)]
    pub fn composite(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        mesh_buffers: (&wgpu::Buffer, &wgpu::Buffer),
        paint: &PaintCaches,
        set: usize,
        state: &mut GpuComposite,
        outputs: &[wgpu::Texture],
        changed: &Region,
        stroke_in_progress: bool,
    ) {
        let texture_set = &model.texture_sets[set];
        let size = state.resolution;
        let signature = signature(&texture_set.stack);
        if state.signature != Some(signature) {
            state.signature = Some(signature);
            state.pending.fill(true);
        }
        let mut now = Region::default();
        if !changed.is_empty() {
            now.extend(&changed.grown(PADDING_MARGIN, size));
            state.stroke_region.extend(changed);
        }
        if !stroke_in_progress && !state.stroke_region.is_empty() {
            now.extend(&state.stroke_region.grown(STROKE_CATCH_UP, size));
            // The padding farther out follows a few chunks per frame.
            let stroke = std::mem::take(&mut state.stroke_region);
            for rect in &stroke.rects {
                state.mark_pending(rect.grow(CHUNK, size));
            }
        }
        let mut rects: Vec<TexelRect> = Vec::new();
        for rect in &now.rects {
            rects.extend(split(*rect, CHUNK));
        }
        if !stroke_in_progress {
            let count = state.pending.len();
            let mut done = 0;
            for step in 0..count {
                if done == CHUNKS_PER_FRAME {
                    break;
                }
                let i = (state.cursor + step) % count;
                if state.pending[i] {
                    state.pending[i] = false;
                    rects.push(state.chunk_rect(i));
                    done += 1;
                    state.cursor = (i + 1) % count;
                }
            }
        }
        if rects.is_empty() {
            return;
        }

        let program = Builder {
            paint,
            resolution: size,
            program: Program::default(),
            current: Vec::new(),
            mask_slots: HashMap::new(),
            save_level: 0,
        }
        .build(&texture_set.stack);
        if program.triplanar && state.triangles.is_none() {
            state.triangles =
                Some(self.compute_triangles(device, queue, model, mesh, mesh_buffers, set, size));
        }
        self.ensure_temps(device, &program);
        self.upload_images(device, queue, model.texture_sets.len(), set, &program);
        self.run(
            device,
            queue,
            model,
            mesh_buffers,
            paint,
            state,
            &mut Target::Channels(outputs),
            &program,
            &rects,
        );
        let mut mips = Region::default();
        for rect in &rects {
            mips.add(*rect);
        }
        self.build_mips(device, queue, outputs, &mips, size);
    }

    /// Composites the whole texture set for the export, a chunk at a time, and hands over
    /// every chunk's texels. The channel textures stay as they are. Fails if the GPU could
    /// not hand the texels back.
    #[allow(clippy::too_many_arguments)]
    pub fn export(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        mesh_buffers: (&wgpu::Buffer, &wgpu::Buffer),
        paint: &PaintCaches,
        set: usize,
        state: &mut GpuComposite,
        sink: &mut dyn FnMut(ExportBlock),
    ) -> Result<(), String> {
        let size = state.resolution;
        let program = Builder {
            paint,
            resolution: size,
            program: Program::default(),
            current: Vec::new(),
            mask_slots: HashMap::new(),
            save_level: 0,
        }
        .build(&model.texture_sets[set].stack);
        if program.triplanar && state.triangles.is_none() {
            state.triangles =
                Some(self.compute_triangles(device, queue, model, mesh, mesh_buffers, set, size));
        }
        self.ensure_temps(device, &program);
        self.upload_images(device, queue, model.texture_sets.len(), set, &program);
        let rects: Vec<TexelRect> = (0..state.pending.len())
            .map(|i| state.chunk_rect(i))
            .collect();
        let readback = Readback::new(device);
        self.run(
            device,
            queue,
            model,
            mesh_buffers,
            paint,
            state,
            &mut Target::Export(&readback, sink),
            &program,
            &rects,
        );
        match readback.error.into_inner() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn ensure_temps(&mut self, device: &wgpu::Device, program: &Program) {
        let texture = |label: &str, format, layers: u32| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: TEMP,
                    height: TEMP,
                    depth_or_array_layers: layers,
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
        let temps = self.temps.get_or_insert_with(|| {
            let results = texture("composite results", ACC_FORMAT, Channel::COUNT as u32);
            Temps {
                acc: Vec::new(),
                saves: Vec::new(),
                mask_tmp: [0, 1].map(|_| texture("composite mask", MASK_FORMAT, 1)),
                masks: Vec::new(),
                results_view: results.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                }),
                results,
                geometry: GEOMETRY_FORMATS.map(|f| texture("composite geometry", f, 1)),
            }
        });
        while temps.acc.len() < program.depth.max(1) {
            temps
                .acc
                .push([0, 1].map(|_| texture("composite accumulator", ACC_FORMAT, 1)));
        }
        while temps.saves.len() < program.saves {
            temps.saves.push(texture("composite save", ACC_FORMAT, 1));
        }
        while temps.masks.len() < program.masks {
            temps.masks.push(texture("composite mask", MASK_FORMAT, 1));
        }
    }

    /// Uploads the images the program uses; drops images no texture set uses any more.
    fn upload_images(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sets: usize,
        set: usize,
        program: &Program,
    ) {
        let mut used = HashSet::new();
        for step in &program.steps {
            let Step::Pass(pass) = step else {
                continue;
            };
            let Some((image, srgb)) = &pass.image else {
                continue;
            };
            let key = (image.id, *srgb);
            used.insert(key);
            self.images
                .entry(key)
                .or_insert_with(|| upload_image(device, queue, image, *srgb));
        }
        self.set_images.resize_with(sets, HashSet::new);
        self.set_images[set] = used;
        let set_images = &self.set_images;
        self.images
            .retain(|key, _| set_images.iter().any(|keys| keys.contains(key)));
    }

    /// Composites the chunks.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh_buffers: (&wgpu::Buffer, &wgpu::Buffer),
        paint: &PaintCaches,
        state: &GpuComposite,
        target: &mut Target,
        program: &Program,
        rects: &[TexelRect],
    ) {
        let passes: Vec<&PassStep> = program
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Pass(p) => Some(&**p),
                _ => None,
            })
            .collect();
        let needed = (passes.len().max(1) as u64) * PASS_STRIDE;
        if needed > self.pass_buffer.size() {
            self.pass_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("composite passes"),
                size: needed.next_power_of_two(),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let temps = self.temps.as_ref().expect("made by ensure_temps");
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let acc_views: Vec<[wgpu::TextureView; 2]> = temps
            .acc
            .iter()
            .map(|pair| pair.each_ref().map(view))
            .collect();
        let save_views: Vec<wgpu::TextureView> = temps.saves.iter().map(view).collect();
        let mask_tmp_views = temps.mask_tmp.each_ref().map(view);
        let mask_views: Vec<wgpu::TextureView> = temps.masks.iter().map(view).collect();
        let geometry_views = temps.geometry.each_ref().map(view);
        let texture_of = |tex: Tex| -> &wgpu::Texture {
            match tex {
                Tex::Acc(d, i) => &temps.acc[d][i],
                Tex::Save(l) => &temps.saves[l],
                Tex::MaskTmp(i) => &temps.mask_tmp[i],
                Tex::MaskCache(k) => &temps.masks[k],
            }
        };
        let view_of = |tex: Tex| -> &wgpu::TextureView {
            match tex {
                Tex::Acc(d, i) => &acc_views[d][i],
                Tex::Save(l) => &save_views[l],
                Tex::MaskTmp(i) => &mask_tmp_views[i],
                Tex::MaskCache(k) => &mask_views[k],
            }
        };

        let geometry = if program.triplanar {
            &geometry_views
        } else {
            &self.dummy_geometry
        };
        let static_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite static"),
            layout: &self.static_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&state.seeds),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.image_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&geometry[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&geometry[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&geometry[2]),
                },
            ],
        });
        let pass_groups: Vec<wgpu::BindGroup> = passes
            .iter()
            .map(|p| {
                let (tiles, slots) =
                    match p.paint.and_then(|(id, pool)| Some((paint.map(id)?, pool))) {
                        Some((map, pool)) => (paint.live[pool as usize].view(), &map.slot_buffer),
                        None => (&self.dummy_tiles, &self.dummy_slots),
                    };
                let image = match &p.image {
                    Some((image, srgb)) => &self.images[&(image.id, *srgb)],
                    None => &self.dummy_image,
                };
                let is_mask = matches!(p.target, Tex::MaskTmp(_) | Tex::MaskCache(_));
                let dummy = if is_mask {
                    &self.dummy_mask
                } else {
                    &self.dummy_acc
                };
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("composite pass"),
                    layout: &self.pass_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &self.pass_buffer,
                                offset: 0,
                                size: wgpu::BufferSize::new(size_of::<PassUniforms>() as u64),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(view_of(p.below)),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(
                                p.other.map_or(dummy, view_of),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(
                                p.mask.map_or(&self.dummy_mask, view_of),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(tiles),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: slots.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::TextureView(image),
                        },
                    ],
                })
            })
            .collect();
        let resolve_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite resolve"),
            layout: &self.resolve_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.resolve_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&temps.results_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&state.seeds),
                },
            ],
        });
        let geometry_group = match (&state.triangles, program.triplanar) {
            (Some(triangles), true) => Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("composite geometry"),
                layout: &self.geometry_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.geometry_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&state.seeds),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(triangles),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: mesh_buffers.0.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: mesh_buffers.1.as_entire_binding(),
                    },
                ],
            })),
            _ => None,
        };
        let output_views: Vec<wgpu::TextureView> = match target {
            Target::Channels(outputs) => outputs
                .iter()
                .map(|t| {
                    t.create_view(&wgpu::TextureViewDescriptor {
                        base_mip_level: 0,
                        mip_level_count: Some(1),
                        ..Default::default()
                    })
                })
                .collect(),
            Target::Export(..) => Vec::new(),
        };
        // Tri-planar projections work in the model's bounds, so a scale of 1 covers the
        // model once along its largest side, as in Painter.
        let bounds = model.bounds();
        let extent = (bounds.max - bounds.min).max_element().max(1e-6);
        let center = (bounds.min + bounds.max) * 0.5;

        for rect in rects {
            let origin = [
                rect.min[0] as i32 - APRON as i32,
                rect.min[1] as i32 - APRON as i32,
            ];
            let w = rect.max[0] - rect.min[0] + 2 * APRON;
            let h = rect.max[1] - rect.min[1] + 2 * APRON;
            let mut bytes = vec![0u8; (passes.len().max(1) as u64 * PASS_STRIDE) as usize];
            for (i, p) in passes.iter().enumerate() {
                let mut uniforms = p.uniforms;
                uniforms.area[0] = origin[0];
                uniforms.area[1] = origin[1];
                uniforms.area[2] = state.resolution as i32;
                let offset = i * PASS_STRIDE as usize;
                bytes[offset..offset + size_of::<PassUniforms>()]
                    .copy_from_slice(bytemuck::bytes_of(&uniforms));
            }
            queue.write_buffer(&self.pass_buffer, 0, &bytes);
            queue.write_buffer(
                &self.resolve_buffer,
                0,
                bytemuck::bytes_of(&ResolveUniforms {
                    area: [origin[0], origin[1], state.resolution as i32, 0],
                    params: [HEIGHT_FACTOR, 0.0, 0.0, 0.0],
                    base_color: background(Channel::BaseColor),
                    surface: [
                        background(Channel::Roughness)[0],
                        background(Channel::Metallic)[0],
                        background(Channel::Opacity)[0],
                        background(Channel::Height)[0],
                    ],
                }),
            );
            queue.write_buffer(
                &self.geometry_buffer,
                0,
                bytemuck::bytes_of(&GeometryUniforms {
                    area: [origin[0], origin[1], state.resolution as i32, 0],
                    bounds: [center.x, center.y, center.z, 1.0 / extent],
                }),
            );

            let mut encoder = device.create_command_encoder(&Default::default());
            let scissor = |pass: &mut wgpu::RenderPass| pass.set_scissor_rect(0, 0, w, h);
            if let Some(group) = &geometry_group {
                let attachments = geometry_views.each_ref().map(|view| {
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
                    label: Some("composite geometry"),
                    color_attachments: &attachments,
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                scissor(&mut pass);
                pass.set_pipeline(&self.geometry);
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..3, 0..1);
            }
            let mut pass_index = 0;
            for step in &program.steps {
                match step {
                    Step::Clear(tex, color) => {
                        // Cleared whole: the area is at most the temp texture.
                        begin_pass(&mut encoder, view_of(*tex), Some(*color));
                    }
                    Step::Copy(from, to) => {
                        encoder.copy_texture_to_texture(
                            texture_of(*from).as_image_copy(),
                            texture_of(*to).as_image_copy(),
                            wgpu::Extent3d {
                                width: w,
                                height: h,
                                depth_or_array_layers: 1,
                            },
                        );
                    }
                    Step::Result(from, channel) => {
                        encoder.copy_texture_to_texture(
                            texture_of(*from).as_image_copy(),
                            wgpu::TexelCopyTextureInfo {
                                origin: wgpu::Origin3d {
                                    x: 0,
                                    y: 0,
                                    z: channel.index() as u32,
                                },
                                ..temps.results.as_image_copy()
                            },
                            wgpu::Extent3d {
                                width: w,
                                height: h,
                                depth_or_array_layers: 1,
                            },
                        );
                    }
                    Step::Pass(p) => {
                        let mut pass = begin_pass(&mut encoder, view_of(p.target), None);
                        scissor(&mut pass);
                        pass.set_pipeline(&self.pipes[&p.pipe.key()]);
                        pass.set_bind_group(0, &static_group, &[]);
                        pass.set_bind_group(
                            1,
                            &pass_groups[pass_index],
                            &[(pass_index as u64 * PASS_STRIDE) as u32],
                        );
                        pass.draw(0..3, 0..1);
                        pass_index += 1;
                    }
                }
            }
            match target {
                Target::Export(readback, sink) => {
                    let w = rect.max[0] - rect.min[0];
                    let h = rect.max[1] - rect.min[1];
                    readback.resolve(&mut encoder, &self.export, &resolve_group, w, h);
                    queue.submit([encoder.finish()]);
                    match readback.read(device, *rect) {
                        Ok(block) => sink(block),
                        Err(err) => {
                            *readback.error.borrow_mut() = Some(err);
                            return;
                        }
                    }
                    continue;
                }
                Target::Channels(_) => {
                    let attachments: Vec<_> = output_views
                        .iter()
                        .map(|view| {
                            Some(wgpu::RenderPassColorAttachment {
                                view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Load,
                                    store: wgpu::StoreOp::Store,
                                },
                            })
                        })
                        .collect();
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("composite resolve"),
                        color_attachments: &attachments,
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    rect.scissor(&mut pass);
                    pass.set_pipeline(&self.resolve);
                    pass.set_bind_group(0, &resolve_group, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
            queue.submit([encoder.finish()]);
        }
    }

    /// Rebuilds the mips of the outputs under `region`.
    fn build_mips(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        outputs: &[wgpu::Texture],
        region: &Region,
        size: u32,
    ) {
        let level_view = |t: &wgpu::Texture, level: u32| {
            t.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        for texture in outputs {
            let pipeline = &self.mip_pipelines[&texture.format()];
            for level in 1..texture.mip_level_count() {
                let source = level_view(texture, level - 1);
                let target = level_view(texture, level);
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
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &group, &[]);
                for rect in &region.rects {
                    rect.at_level(level, size).scissor(&mut pass);
                    pass.draw(0..3, 0..1);
                }
            }
        }
        queue.submit([encoder.finish()]);
    }

    /// For every texel, the nearest texel covered by the texture set's triangles.
    fn compute_seeds(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        set: usize,
        resolution: u32,
    ) -> wgpu::TextureView {
        let textures = [0, 1].map(|_| seed_texture(device, resolution, "paint seeds"));
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

    /// For every texel covered by the texture set's triangles, the index of its triangle.
    #[allow(clippy::too_many_arguments)]
    fn compute_triangles(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        _mesh: &GpuMesh,
        mesh_buffers: (&wgpu::Buffer, &wgpu::Buffer),
        set: usize,
        resolution: u32,
    ) -> wgpu::TextureView {
        let view =
            seed_texture(device, resolution, "triangle ids").create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("triangle ids"),
            layout: &self.triangles_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: mesh_buffers.0.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: mesh_buffers.1.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = begin_pass(
                &mut encoder,
                &view,
                Some(wgpu::Color {
                    r: u32::MAX as f64,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                }),
            );
            pass.set_pipeline(&self.triangles);
            pass.set_bind_group(0, &group, &[]);
            for part in model.parts.iter().filter(|p| p.texture_set == set) {
                pass.draw(part.indices.clone(), 0..1);
            }
        }
        queue.submit([encoder.finish()]);
        view
    }
}

/// Strength of normals derived from height: Painter's HEIGHT_FACTOR.
const HEIGHT_FACTOR: f32 = 400.0;

fn seed_texture(device: &wgpu::Device, resolution: u32, label: &str) -> wgpu::Texture {
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
        format: SEED_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// Splits a rectangle into pieces of at most `max` texels per side.
fn split(rect: TexelRect, max: u32) -> Vec<TexelRect> {
    let mut pieces = Vec::new();
    if rect.is_empty() {
        return pieces;
    }
    let mut y = rect.min[1];
    while y < rect.max[1] {
        let y1 = (y + max).min(rect.max[1]);
        let mut x = rect.min[0];
        while x < rect.max[0] {
            let x1 = (x + max).min(rect.max[0]);
            pieces.push(TexelRect {
                min: [x, y],
                max: [x1, y1],
            });
            x = x1;
        }
        y = y1;
    }
    pieces
}

/// Hash of what in the stack shapes the composite, except the strokes, which the paint
/// caches track.
fn signature(stack: &LayerStack) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    fn floats(h: &mut impl Hasher, values: &[f32]) {
        for v in values {
            v.to_bits().hash(h);
        }
    }
    fn projection(h: &mut impl Hasher, p: &Projection) {
        (p.mode as u8).hash(h);
        floats(h, &p.scale);
        floats(h, &[p.rotation, p.hardness]);
        floats(h, &p.offset);
    }
    fn image(h: &mut impl Hasher, image: Option<&Arc<TextureImage>>) {
        image.map(|i| i.id).hash(h);
    }
    fn layers(h: &mut impl Hasher, list: &[Layer]) {
        list.len().hash(h);
        for layer in list {
            layer.id.hash(h);
            layer.visible.hash(h);
            for channel in Channel::ALL {
                let b = layer.blending.get(channel);
                b.mode.code().hash(h);
                b.opacity.to_bits().hash(h);
            }
            match &layer.content {
                Content::Paint(p) => (0u8, p.id).hash(h),
                Content::Fill(f) => {
                    1u8.hash(h);
                    for channel in Channel::ALL {
                        f.channels.contains(channel).hash(h);
                        floats(h, &f.values.get(channel));
                        image(h, f.image(channel));
                    }
                    f.normal_directx.hash(h);
                    projection(h, &f.projection);
                }
                Content::Folder(folder) => {
                    2u8.hash(h);
                    layers(h, &folder.layers);
                }
            }
            match &layer.mask {
                None => 0u8.hash(h),
                Some(mask) => {
                    1u8.hash(h);
                    mask.enabled.hash(h);
                    mask.background.to_bits().hash(h);
                    mask.paint.id.hash(h);
                    for effect in &mask.effects {
                        effect.id.hash(h);
                        effect.visible.hash(h);
                        effect.blend.code().hash(h);
                        effect.opacity.to_bits().hash(h);
                        match &effect.kind {
                            EffectKind::Paint(p) => (0u8, p.id).hash(h),
                            EffectKind::Fill(g) => {
                                1u8.hash(h);
                                g.value.to_bits().hash(h);
                                image(h, g.image.as_ref());
                                projection(h, &g.projection);
                            }
                            EffectKind::Levels(l) => {
                                2u8.hash(h);
                                floats(h, &l.input);
                                floats(h, &[l.gamma]);
                                floats(h, &l.output);
                            }
                        }
                    }
                }
            }
        }
    }
    layers(&mut h, &stack.layers);
    h.finish()
}

/// Uploads a fill image with mips. Color images are sRGB-encoded and decoded when sampled.
fn upload_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &TextureImage,
    srgb: bool,
) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    let limit = device.limits().max_texture_dimension_2d;
    let (format, levels): (wgpu::TextureFormat, Vec<(u32, u32, Vec<u8>)>) = match &image.texels {
        Texels::Rgba8(pixels) => {
            let format = if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            };
            // Average in linear space for color images.
            let to_linear: fn(u8) -> f32 = if srgb {
                |v| srgb_to_linear(v as f32 / 255.0)
            } else {
                |v| v as f32 / 255.0
            };
            let from_linear: fn(f32) -> u8 = if srgb {
                |v| (linear_to_srgb(v) * 255.0).round() as u8
            } else {
                |v| (v * 255.0).round() as u8
            };
            let base: Vec<f32> = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    [
                        to_linear(p[0]),
                        to_linear(p[1]),
                        to_linear(p[2]),
                        p[3] as f32 / 255.0,
                    ]
                })
                .collect();
            let levels = mip_chain(image.width, image.height, base)
                .into_iter()
                .map(|(w, h, data)| {
                    let bytes = data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|p| {
                            [
                                from_linear(p[0]),
                                from_linear(p[1]),
                                from_linear(p[2]),
                                (p[3] * 255.0).round() as u8,
                            ]
                        })
                        .collect();
                    (w, h, bytes)
                })
                .collect();
            (format, levels)
        }
        Texels::Rgba16(pixels) => {
            // Decoded on the CPU into half floats, linear for colors.
            let decode = |v: u16| {
                let v = v as f32 / 65535.0;
                if srgb { srgb_to_linear(v) } else { v }
            };
            let base: Vec<f32> = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    [
                        decode(p[0]),
                        decode(p[1]),
                        decode(p[2]),
                        p[3] as f32 / 65535.0,
                    ]
                })
                .collect();
            let levels = mip_chain(image.width, image.height, base)
                .into_iter()
                .map(|(w, h, data)| {
                    let halves: Vec<half::f16> =
                        data.iter().map(|&v| half::f16::from_f32(v)).collect();
                    (w, h, bytemuck::cast_slice(&halves).to_vec())
                })
                .collect();
            (wgpu::TextureFormat::Rgba16Float, levels)
        }
        Texels::RgbaF32(pixels) => {
            let levels = mip_chain(image.width, image.height, pixels.clone())
                .into_iter()
                .map(|(w, h, data)| {
                    let halves: Vec<half::f16> =
                        data.iter().map(|&v| half::f16::from_f32(v)).collect();
                    (w, h, bytemuck::cast_slice(&halves).to_vec())
                })
                .collect();
            (wgpu::TextureFormat::Rgba16Float, levels)
        }
    };
    let first = levels
        .iter()
        .position(|(w, h, _)| *w <= limit && *h <= limit)
        .unwrap_or(levels.len() - 1);
    let levels = &levels[first..];
    let (width, height, _) = levels[0];
    let data: Vec<u8> = levels
        .iter()
        .flat_map(|(_, _, d)| d.iter().copied())
        .collect();
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(&image.name),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::MipMajor,
            &data,
        )
        .create_view(&Default::default())
}

/// Box-filtered mip levels of RGBA float pixels, down to 1x1.
fn mip_chain(width: u32, height: u32, base: Vec<f32>) -> Vec<(u32, u32, Vec<f32>)> {
    let mut levels = vec![(width.max(1), height.max(1), base)];
    while let Some((w, h, data)) = levels.last()
        && (*w > 1 || *h > 1)
    {
        let (w, h) = (*w, *h);
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0.0; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(w - 1);
                    let sy = (y * 2 + dy).min(h - 1);
                    let from = ((sy * w + sx) * 4) as usize;
                    let to = ((y * nw + x) * 4) as usize;
                    for k in 0..4 {
                        next[to + k] += data[from + k] * 0.25;
                    }
                }
            }
        }
        levels.push((nw, nh, next));
    }
    levels
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ink_core::{Ids, LayerKind};

    #[test]
    fn layouts_match_shaders() {
        assert_eq!(size_of::<PassUniforms>(), 128);
        assert!(size_of::<PassUniforms>() as u64 <= PASS_STRIDE);
        assert_eq!(size_of::<ResolveUniforms>(), 64);
        assert_eq!(size_of::<GeometryUniforms>(), 32);
        let bytes: u32 = GEOMETRY_FORMATS
            .iter()
            .map(|f| f.block_copy_size(None).unwrap())
            .sum();
        assert!(bytes <= 32, "color attachment bytes per sample");
    }

    #[test]
    fn blend_codes_match_the_shader() {
        let source = include_str!("composite.wgsl");
        for mode in BlendMode::ALL {
            let name: String = format!("{mode:?}")
                .chars()
                .enumerate()
                .flat_map(|(i, c)| {
                    let sep = (i > 0 && c.is_uppercase()).then_some('_');
                    sep.into_iter().chain(c.to_uppercase())
                })
                .collect();
            let line = format!("const BLEND_{name}: u32 = {}u;", mode.code());
            assert!(source.contains(&line), "missing {line}");
        }
    }

    #[test]
    fn rectangles_split_into_chunks() {
        let pieces = split(
            TexelRect {
                min: [10, 0],
                max: [1100, 600],
            },
            512,
        );
        assert_eq!(pieces.len(), 6);
        let texels: u64 = pieces.iter().map(TexelRect::texels).sum();
        assert_eq!(texels, 1090 * 600);
        assert!(
            pieces
                .iter()
                .all(|p| p.max[0] - p.min[0] <= 512 && p.max[1] - p.min[1] <= 512)
        );
    }

    #[test]
    fn signatures_ignore_strokes_but_not_settings() {
        let mut ids = Ids::default();
        let mut stack = LayerStack::new_default(&mut ids);
        let before = signature(&stack);
        stack.layers[0].blending.get_mut(Channel::Roughness).opacity = 0.5;
        assert_ne!(signature(&stack), before);
        let changed = signature(&stack);
        stack
            .layers
            .push(Layer::new(LayerKind::Fill, "Fill layer 1".into(), &mut ids));
        assert_ne!(signature(&stack), changed);
    }

    #[test]
    fn mips_average_down_to_one_texel() {
        let levels = mip_chain(4, 2, (0..32).map(|i| i as f32).collect());
        assert_eq!(levels.len(), 3);
        assert_eq!((levels[1].0, levels[1].1), (2, 1));
        assert_eq!((levels[2].0, levels[2].1), (1, 1));
        let first: f32 = (0..32).step_by(4).map(|i| i as f32).sum::<f32>() / 8.0;
        assert!((levels[2].2[0] - first).abs() < 1e-4);
    }
}
