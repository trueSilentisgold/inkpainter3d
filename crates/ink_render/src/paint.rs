//! Brush strokes on the GPU. Every paint layer (the content of a paint layer, a mask, a
//! paint effect) keeps its painted pixels in sparse tiles (see tiles.rs): only tiles that
//! were painted take memory. A stroke's dabs accumulate in a coverage texture, which is
//! then blended into the tiles of each channel the stroke paints.
//!
//! The renderer keeps track of which strokes each paint layer holds and only rasterizes
//! what changed: new dabs of the stroke being painted, new strokes, or, after undo, the
//! tiles as they were before the undone strokes. Anything else repaints the layer's
//! strokes. Before the topmost stroke touches a tile, the tile is copied into a snapshot
//! pool: the stroke is then redrawn over that copy as it grows, and undone by copying it
//! back. Copies of earlier strokes are kept within a memory budget for fast undo.

use crate::region::{Region, TexelRect};
use crate::tiles::{EMPTY, TILE, TilePool, slot_place};
use crate::viewport::{GpuMesh, srgb_to_linear};
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use ink_core::{
    Alpha, Channel, ChannelKind, Content, Dab, EffectKind, Model, Stroke, StrokeCamera,
    StrokeSpace, Tool,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

const COVERAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// Tile formats: color channels and normals, scalar channels and masks. Premultiplied:
/// rgb (or r) is value * coverage, alpha (or g) the coverage.
pub(crate) const POOL_FORMATS: [wgpu::TextureFormat; 2] = [
    wgpu::TextureFormat::Rgba16Float,
    wgpu::TextureFormat::Rg16Float,
];
const MAX_DABS_PER_DRAW: usize = 64;
/// Longer side of the depth maps used for occlusion, in pixels. Fixed, so strokes paint the
/// same whatever the window size.
const DEPTH_MAP_SIZE: u32 = 2048;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// Depth maps kept for strokes painted from the same view.
const DEPTH_MAP_CACHE: usize = 8;
/// Video memory for the tiles strokes painted over, kept for fast undo.
const PRE_IMAGE_BUDGET: u64 = 512 << 20;
/// At most this many strokes keep their pre-images.
const PRE_IMAGES_KEPT: usize = 1024;
/// Paint layers that left their stack (deleted, or undone) keep their tiles for a while,
/// in case they come back with undo.
const ORPHANS_KEPT: usize = 32;
/// Video memory the tiles of those paint layers may take together.
const ORPHAN_BUDGET: u64 = 512 << 20;
/// Uniform buffer offset alignment for the per-channel apply parameters.
const APPLY_STRIDE: u64 = 256;

/// Which pool a channel's tiles live in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PoolKind {
    Rgba = 0,
    Rg = 1,
}

impl PoolKind {
    /// Components the tiles hold.
    fn components(self) -> f32 {
        match self {
            Self::Rgba => 4.0,
            Self::Rg => 2.0,
        }
    }
}

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

/// Must match `ApplyParams` in paint_tiles.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ApplyParams {
    value: [f32; 4],
    info: [f32; 4],
}

/// Must match `TileDraw` in paint_tiles.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TileDraw {
    dest: [u32; 4],
    before: [u32; 4],
}

/// The painted pixels of one paint layer: per channel, a slot of the channel's pool for
/// every tile, or [`EMPTY`].
pub(crate) struct PaintMap {
    pub set: usize,
    pub resolution: u32,
    /// A mask or paint effect: one grayscale channel instead of the texture set channels.
    pub gray: bool,
    /// Tiles per side of the texture.
    pub tiles_side: u32,
    /// Per channel ([`Channel::ALL`] order, or just one when gray), the slot of each tile,
    /// row by row.
    pub slots: Vec<Vec<u32>>,
    /// `slots` on the GPU, channel after channel, for the compositor.
    pub slot_buffer: wgpu::Buffer,
    slots_dirty: bool,
    /// Strokes the tiles hold, with their number of dabs, bottom to top.
    applied: Vec<(u64, usize)>,
    /// When the map's paint layer was last part of its stack; see [`ORPHANS_KEPT`].
    last_seen: u64,
}

impl PaintMap {
    fn new(device: &wgpu::Device, set: usize, resolution: u32, gray: bool) -> Self {
        let tiles_side = resolution.div_ceil(TILE).max(1);
        let channels = if gray { 1 } else { Channel::COUNT };
        let tiles = (tiles_side * tiles_side) as usize;
        let slot_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("paint tile slots"),
            size: (channels * tiles * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            set,
            resolution,
            gray,
            tiles_side,
            slots: vec![vec![EMPTY; tiles]; channels],
            slot_buffer,
            slots_dirty: true,
            applied: Vec::new(),
            last_seen: 0,
        }
    }

    /// The pool holding the tiles of a channel (an index into `slots`).
    pub fn pool(&self, channel: usize) -> PoolKind {
        pool_kind(self.gray, channel)
    }

    /// Whether any tile of the channel holds paint.
    pub fn has_paint(&self, channel: usize) -> bool {
        self.slots
            .get(channel)
            .is_some_and(|s| s.iter().any(|&slot| slot != EMPTY))
    }

    /// The texels of a tile.
    fn tile_rect(&self, tile: u32) -> TexelRect {
        let (x, y) = (tile % self.tiles_side, tile / self.tiles_side);
        TexelRect {
            min: [x * TILE, y * TILE],
            max: [
                ((x + 1) * TILE).min(self.resolution),
                ((y + 1) * TILE).min(self.resolution),
            ],
        }
    }

    /// Tiles that overlap the region.
    fn tiles_in(&self, region: &Region) -> Vec<u32> {
        let mut tiles = HashSet::new();
        for rect in &region.rects {
            if rect.is_empty() {
                continue;
            }
            for y in rect.min[1] / TILE..rect.max[1].div_ceil(TILE).min(self.tiles_side) {
                for x in rect.min[0] / TILE..rect.max[0].div_ceil(TILE).min(self.tiles_side) {
                    tiles.insert(y * self.tiles_side + x);
                }
            }
        }
        let mut tiles: Vec<u32> = tiles.into_iter().collect();
        tiles.sort_unstable();
        tiles
    }

    /// The channels a stroke paints, as indices into `slots`.
    fn channels_of(&self, stroke: &Stroke) -> Vec<usize> {
        if self.gray {
            vec![0]
        } else {
            stroke
                .material
                .channels
                .iter()
                .map(Channel::index)
                .collect()
        }
    }
}

/// The topmost stroke of a paint layer and the tiles it painted over, as they were before.
struct Snapshot {
    paint: u64,
    stroke: u64,
    resolution: u32,
    /// Whether the paint layer is grayscale, which decides the pools of its tiles.
    gray: bool,
    /// (channel, tile) to the slot of the copy in the snapshot pool, or [`EMPTY`] if the
    /// tile held no paint.
    before: HashMap<(usize, u32), u32>,
}

impl Snapshot {
    /// The copies in the snapshot pool, with their pool.
    fn copies(&self) -> impl Iterator<Item = (PoolKind, u32)> + '_ {
        self.before
            .iter()
            .filter(|&(_, &slot)| slot != EMPTY)
            .map(|(&(channel, _), &slot)| (pool_kind(self.gray, channel), slot))
    }
}

/// The pool of a channel's tiles; grayscale maps have one channel.
pub(crate) fn pool_kind(gray: bool, channel: usize) -> PoolKind {
    if gray {
        return PoolKind::Rg;
    }
    match Channel::ALL[channel].kind() {
        ChannelKind::Color | ChannelKind::Normal => PoolKind::Rgba,
        ChannelKind::Scalar => PoolKind::Rg,
    }
}

/// How a tile draw combines the stroke with the tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Mode {
    /// Writes the whole tile: its snapshot (or nothing) with the stroke on top.
    Replace,
    Over,
    Erase,
}

/// A coverage texture and the resolution it was made for.
struct Coverage {
    resolution: u32,
    view: wgpu::TextureView,
}

/// What a paint layer is in its stack.
#[derive(Clone, Copy)]
struct PaintRole {
    id: u64,
    gray: bool,
}

pub(crate) struct PaintCaches {
    depth: wgpu::RenderPipeline,
    depth_layout: wgpu::BindGroupLayout,
    depth_maps: Vec<DepthMap>,
    depth_uses: u64,
    /// Bound when a stroke has no camera.
    no_depth: wgpu::TextureView,
    dab_layout: wgpu::BindGroupLayout,
    dabs_surface: wgpu::RenderPipeline,
    dabs_uv: wgpu::RenderPipeline,
    tile_layout: wgpu::BindGroupLayout,
    /// Per pool kind: writes whole tiles from a snapshot; paints over; erases.
    tile_replace: [wgpu::RenderPipeline; 2],
    tile_over: [wgpu::RenderPipeline; 2],
    tile_erase: [wgpu::RenderPipeline; 2],
    alpha_sampler: wgpu::Sampler,
    white_alpha: wgpu::TextureView,
    alphas: HashMap<u64, wgpu::TextureView>,
    stroke_buffer: wgpu::Buffer,
    dab_buffer: wgpu::Buffer,
    apply_buffer: wgpu::Buffer,
    draw_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    /// Marks triangles already selected for the current draw.
    triangle_marks: Vec<u32>,
    mark: u32,
    /// Tiles of the paint layers, by [`PoolKind`].
    pub live: [TilePool; 2],
    /// Copies of tiles before strokes painted over them, by [`PoolKind`].
    snap: [TilePool; 2],
    /// By paint layer id.
    pub maps: HashMap<u64, PaintMap>,
    snapshot: Option<Snapshot>,
    /// Oldest first; their copies together take at most [`PRE_IMAGE_BUDGET`].
    pre_images: VecDeque<Snapshot>,
    /// The topmost stroke's coverage, kept while it grows.
    coverage_live: Option<Coverage>,
    /// For strokes painted at once.
    coverage_scratch: Option<Coverage>,
    syncs: u64,
}

impl PaintCaches {
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
        let dabs_shader = module("paint dabs", include_str!("paint_dabs.wgsl"));
        let tiles_shader = module("paint tiles", include_str!("paint_tiles.wgsl"));
        let depth_shader = module("paint depth", include_str!("paint_depth.wgsl"));

        let texture = |binding, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let d2 = wgpu::TextureViewDimension::D2;
        let float = |filterable| wgpu::TextureSampleType::Float { filterable };
        let buffer = |binding, ty, has_dynamic_offset, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset,
                min_binding_size: None,
            },
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
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let dab_layout = layout(
            "paint dabs",
            &[
                buffer(0, uniform, false, fragment),
                buffer(1, storage, false, fragment),
                texture(2, float(true), d2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture(4, float(false), d2),
            ],
        );
        let depth_layout = layout(
            "paint depth",
            &[buffer(
                0,
                uniform,
                false,
                wgpu::ShaderStages::VERTEX_FRAGMENT,
            )],
        );
        let tile_layout = layout(
            "paint tiles",
            &[
                buffer(0, uniform, true, fragment),
                buffer(1, storage, false, wgpu::ShaderStages::VERTEX_FRAGMENT),
                texture(2, float(false), d2),
                texture(3, float(false), wgpu::TextureViewDimension::D2Array),
            ],
        );

        struct Desc<'a> {
            label: &'a str,
            layout: &'a wgpu::BindGroupLayout,
            shader: &'a wgpu::ShaderModule,
            vs: &'a str,
            fs: &'a str,
            mesh: bool,
            depth: bool,
            target: wgpu::ColorTargetState,
        }
        let pipeline = |d: Desc| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(d.label),
                bind_group_layouts: &[Some(d.layout)],
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
                // Triangles are drawn in UV space, where mirrored islands flip the winding;
                // the depth map is two-sided like the viewport.
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: d.depth.then(|| wgpu::DepthStencilState {
                    format: crate::viewport::DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: d.shader,
                    entry_point: Some(d.fs),
                    compilation_options: Default::default(),
                    targets: &[Some(d.target)],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let target = |format, blend| wgpu::ColorTargetState {
            format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
        };

        let depth = pipeline(Desc {
            label: "paint depth",
            layout: &depth_layout,
            shader: &depth_shader,
            vs: "vs_depth",
            fs: "fs_depth",
            mesh: true,
            depth: true,
            target: target(DEPTH_FORMAT, None),
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
            layout: &dab_layout,
            shader: &dabs_shader,
            vs: "vs_surface",
            fs: "fs_surface",
            mesh: true,
            depth: false,
            target: target(COVERAGE_FORMAT, Some(accumulate)),
        });
        let dabs_uv = pipeline(Desc {
            label: "paint dabs uv",
            layout: &dab_layout,
            shader: &dabs_shader,
            vs: "vs_fullscreen",
            fs: "fs_uv",
            mesh: false,
            depth: false,
            target: target(COVERAGE_FORMAT, Some(accumulate)),
        });

        let erase = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let tile_pipeline = |format, fs, blend| {
            pipeline(Desc {
                label: "paint tiles",
                layout: &tile_layout,
                shader: &tiles_shader,
                vs: "vs_tile",
                fs,
                mesh: false,
                depth: false,
                target: target(format, blend),
            })
        };
        let tile_replace = POOL_FORMATS.map(|f| tile_pipeline(f, "fs_replace", None));
        let tile_over = POOL_FORMATS.map(|f| {
            tile_pipeline(
                f,
                "fs_blend",
                Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            )
        });
        let tile_erase = POOL_FORMATS.map(|f| {
            tile_pipeline(
                f,
                "fs_blend",
                Some(wgpu::BlendState {
                    color: erase,
                    alpha: erase,
                }),
            )
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
        let pool =
            |label, kind: PoolKind| TilePool::new(device, label, POOL_FORMATS[kind as usize]);
        Self {
            depth,
            depth_layout,
            depth_maps: Vec::new(),
            depth_uses: 0,
            no_depth,
            dab_layout,
            dabs_surface,
            dabs_uv,
            tile_layout,
            tile_replace,
            tile_over,
            tile_erase,
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
                APPLY_STRIDE * Channel::COUNT as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
            draw_buffer: buffer(
                "tile draws",
                256 * size_of::<TileDraw>() as u64,
                wgpu::BufferUsages::STORAGE,
            ),
            index_buffer: buffer("dab triangles", 4096 * 12, wgpu::BufferUsages::INDEX),
            triangle_marks: Vec::new(),
            mark: 0,
            live: [
                pool("paint tiles rgba", PoolKind::Rgba),
                pool("paint tiles rg", PoolKind::Rg),
            ],
            snap: [
                pool("paint snapshots rgba", PoolKind::Rgba),
                pool("paint snapshots rg", PoolKind::Rg),
            ],
            maps: HashMap::new(),
            snapshot: None,
            pre_images: VecDeque::new(),
            coverage_live: None,
            coverage_scratch: None,
            syncs: 0,
        }
    }

    /// Forgets everything tied to the previous model.
    pub fn reset(&mut self) {
        for (_, map) in std::mem::take(&mut self.maps) {
            self.release_map(&map);
        }
        if let Some(snapshot) = self.snapshot.take() {
            self.release_copies(&snapshot);
        }
        for pre in std::mem::take(&mut self.pre_images) {
            self.release_copies(&pre);
        }
        self.triangle_marks.clear();
        self.depth_maps.clear();
    }

    /// The tiles of a paint layer.
    pub fn map(&self, paint: u64) -> Option<&PaintMap> {
        self.maps.get(&paint)
    }

    /// Brings the tiles of a texture set's paint layers in line with their strokes. Returns
    /// the texels that changed.
    pub fn sync_set(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        set: usize,
    ) -> Region {
        self.syncs += 1;
        let texture_set = &model.texture_sets[set];
        let resolution = texture_set.resolution.max(1);
        let mut dirty = Region::default();
        let mut seen = HashSet::new();
        for (role, strokes) in paint_roles(&texture_set.stack) {
            seen.insert(role.id);
            let usable = self
                .maps
                .get(&role.id)
                .is_some_and(|m| m.resolution == resolution && m.gray == role.gray && m.set == set);
            if !usable {
                self.forget(role.id);
                if let Some(old) = self.maps.remove(&role.id) {
                    self.release_map(&old);
                }
                let map = PaintMap::new(device, set, resolution, role.gray);
                self.maps.insert(role.id, map);
            }
            let mut map = self.maps.remove(&role.id).expect("inserted above");
            map.last_seen = self.syncs;
            let changed = self.sync_map(device, queue, model, mesh, role.id, &mut map, strokes);
            dirty.extend(&changed);
            self.maps.insert(role.id, map);
        }
        self.drop_orphans(set, &seen);
        for map in self.maps.values_mut().filter(|m| m.slots_dirty) {
            let slots: Vec<u32> = map.slots.concat();
            queue.write_buffer(&map.slot_buffer, 0, bytemuck::cast_slice(&slots));
            map.slots_dirty = false;
        }
        dirty
    }

    /// Paint layers of the set that are no longer in its stack are kept for undo, up to
    /// [`ORPHANS_KEPT`] of them within [`ORPHAN_BUDGET`]; the ones that left first go first.
    fn drop_orphans(&mut self, set: usize, seen: &HashSet<u64>) {
        let mut orphans: Vec<(u64, u64, u64)> = self
            .maps
            .iter()
            .filter(|(id, m)| m.set == set && !seen.contains(id))
            .map(|(&id, m)| (m.last_seen, id, self.map_bytes(m)))
            .collect();
        orphans.sort_unstable();
        let mut bytes: u64 = orphans.iter().map(|o| o.2).sum();
        let mut count = orphans.len();
        for &(_, id, size) in &orphans {
            if count <= ORPHANS_KEPT && bytes <= ORPHAN_BUDGET {
                break;
            }
            self.forget(id);
            if let Some(map) = self.maps.remove(&id) {
                self.release_map(&map);
            }
            count -= 1;
            bytes -= size;
        }
    }

    /// Video memory a paint layer's tiles take.
    fn map_bytes(&self, map: &PaintMap) -> u64 {
        map.slots
            .iter()
            .enumerate()
            .map(|(c, slots)| {
                let tiles = slots.iter().filter(|&&s| s != EMPTY).count() as u64;
                tiles * self.live[map.pool(c) as usize].bytes_per_tile()
            })
            .sum()
    }

    /// Brings one paint layer's tiles in line with its strokes. Returns the texels that
    /// changed.
    #[allow(clippy::too_many_arguments)]
    fn sync_map(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        id: u64,
        map: &mut PaintMap,
        strokes: &[Arc<Stroke>],
    ) -> Region {
        let size = map.resolution;
        let common = map
            .applied
            .iter()
            .zip(strokes)
            .take_while(|((sid, dabs), s)| *sid == s.id && *dabs == s.dabs.len())
            .count();
        // The stroke whose snapshot is at hand, if it belongs to this layer.
        let snapshot = self
            .snapshot
            .as_ref()
            .filter(|s| s.paint == id && s.resolution == size)
            .map(|s| s.stroke);
        let topmost = map.applied.last().map(|&(sid, _)| sid);
        let mut dirty = Region::default();

        if common + 1 == map.applied.len()
            && let Some(stroke) = strokes.get(common)
            && Some(stroke.id) == topmost
            && stroke.dabs.len() > map.applied[common].1
            && snapshot == topmost
        {
            // The stroke being painted got new dabs. Its coverage so far is still in the
            // live coverage texture; the snapshot holds the tiles underneath.
            let done = map.applied[common].1;
            let coverage = coverage_view(&mut self.coverage_live, device, size);
            let region = self.rasterize(
                device,
                queue,
                model,
                mesh,
                map.set,
                size,
                &coverage,
                stroke,
                &stroke.dabs[done..],
            );
            self.apply_live(device, queue, map, stroke, &coverage, &region);
            map.applied[common].1 = stroke.dabs.len();
            dirty = region;
        } else if common < map.applied.len() {
            // Strokes were taken off the top (undo): put back what they painted over. If
            // that is gone, or other strokes changed, paint everything again.
            let undone = strokes.len() == common
                && self.undo_strokes(device, queue, id, map, common, &mut dirty);
            if !undone {
                self.forget(id);
                self.clear_map(map);
                dirty = Region::full(size);
            }
        }

        for (i, stroke) in strokes.iter().enumerate().skip(map.applied.len()) {
            let last = i + 1 == strokes.len();
            let coverage = if last {
                self.start_snapshot(id, stroke.id, size, map.gray);
                coverage_view(&mut self.coverage_live, device, size)
            } else {
                coverage_view(&mut self.coverage_scratch, device, size)
            };
            clear(device, queue, &coverage);
            let region = self.rasterize(
                device,
                queue,
                model,
                mesh,
                map.set,
                size,
                &coverage,
                stroke,
                &stroke.dabs,
            );
            if last {
                self.apply_live(device, queue, map, stroke, &coverage, &region);
            } else {
                self.apply_replay(device, queue, map, stroke, &coverage, &region);
            }
            map.applied.push((stroke.id, stroke.dabs.len()));
            dirty.extend(&region);
        }
        dirty
    }

    /// Redraws the tiles under `region` as the snapshot's tiles with the topmost stroke's
    /// whole coverage on top. Tiles the stroke touches for the first time are copied into
    /// the snapshot first.
    fn apply_live(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: &mut PaintMap,
        stroke: &Stroke,
        coverage: &wgpu::TextureView,
        region: &Region,
    ) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let tiles = map.tiles_in(region);
        let channels = map.channels_of(stroke);
        let erase = stroke.tool == Tool::Erase;
        // Make room first: growing a pool replaces its texture. A tile touched for the first
        // time takes a new tile if it was empty, else a snapshot copy.
        let mut new_tiles = [0u32; 2];
        let mut copies = [0u32; 2];
        for &c in &channels {
            for &t in tiles
                .iter()
                .filter(|&&t| !snapshot.before.contains_key(&(c, t)))
            {
                let kind = map.pool(c) as usize;
                if map.slots[c][t as usize] != EMPTY {
                    copies[kind] += 1;
                } else if !erase {
                    new_tiles[kind] += 1;
                }
            }
        }
        for kind in [PoolKind::Rgba, PoolKind::Rg] {
            self.live[kind as usize].reserve(device, queue, new_tiles[kind as usize]);
            self.snap[kind as usize].reserve(device, queue, copies[kind as usize]);
        }

        let Self {
            snapshot,
            live,
            snap,
            ..
        } = self;
        let snapshot = snapshot.as_mut().expect("checked above");
        let mut encoder = device.create_command_encoder(&Default::default());
        let mut draws = Vec::new();
        for &c in &channels {
            let kind = map.pool(c) as usize;
            for &t in &tiles {
                let before = match snapshot.before.get(&(c, t)) {
                    Some(&before) => before,
                    None => {
                        let current = map.slots[c][t as usize];
                        let before = if current == EMPTY {
                            if erase {
                                // Nothing to erase here.
                                continue;
                            }
                            map.slots[c][t as usize] = live[kind].allocate(device, queue);
                            map.slots_dirty = true;
                            EMPTY
                        } else {
                            let copy = snap[kind].allocate(device, queue);
                            copy_tile(
                                &mut encoder,
                                live[kind].texture(),
                                current,
                                snap[kind].texture(),
                                copy,
                            );
                            copy
                        };
                        snapshot.before.insert((c, t), before);
                        before
                    }
                };
                let slot = map.slots[c][t as usize];
                if slot != EMPTY {
                    draws.push((Mode::Replace, c, slot, tile_draw(map, t, slot, before)));
                }
            }
        }
        self.draw_tiles(device, queue, encoder, map, stroke, coverage, draws);
    }

    /// Puts a stroke onto the tiles under `region` for good: over them, or erasing them.
    fn apply_replay(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: &mut PaintMap,
        stroke: &Stroke,
        coverage: &wgpu::TextureView,
        region: &Region,
    ) {
        let tiles = map.tiles_in(region);
        let channels = map.channels_of(stroke);
        let erase = stroke.tool == Tool::Erase;
        // Only empty tiles take new ones; erasing never does.
        let mut needed = [0u32; 2];
        if !erase {
            for &c in &channels {
                let empty = tiles
                    .iter()
                    .filter(|&&t| map.slots[c][t as usize] == EMPTY)
                    .count() as u32;
                needed[map.pool(c) as usize] += empty;
            }
        }
        for kind in [PoolKind::Rgba, PoolKind::Rg] {
            self.live[kind as usize].reserve(device, queue, needed[kind as usize]);
        }
        let mut draws = Vec::new();
        for &c in &channels {
            let kind = map.pool(c) as usize;
            for &t in &tiles {
                let slot = map.slots[c][t as usize];
                if slot != EMPTY {
                    let mode = if erase { Mode::Erase } else { Mode::Over };
                    draws.push((mode, c, slot, tile_draw(map, t, slot, EMPTY)));
                } else if !erase {
                    // A new tile: written whole, so its old texels don't matter.
                    let slot = self.live[kind].allocate(device, queue);
                    map.slots[c][t as usize] = slot;
                    map.slots_dirty = true;
                    draws.push((Mode::Replace, c, slot, tile_draw(map, t, slot, EMPTY)));
                }
            }
        }
        let encoder = device.create_command_encoder(&Default::default());
        self.draw_tiles(device, queue, encoder, map, stroke, coverage, draws);
    }

    /// Draws tiles grouped by mode, channel and page, then submits the encoder.
    #[allow(clippy::too_many_arguments)]
    fn draw_tiles(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut encoder: wgpu::CommandEncoder,
        map: &PaintMap,
        stroke: &Stroke,
        coverage: &wgpu::TextureView,
        mut draws: Vec<(Mode, usize, u32, TileDraw)>,
    ) {
        if draws.is_empty() {
            queue.submit([encoder.finish()]);
            return;
        }
        draws.sort_by_key(|&(mode, c, slot, _)| (mode, c, slot_place(slot).0));

        // Parameters per channel, at their own offsets.
        let mut bytes = vec![0u8; self.apply_buffer.size() as usize];
        let erase = f32::from(stroke.tool == Tool::Erase);
        for c in 0..map.slots.len() {
            let value = if map.gray {
                [stroke.material.grayscale, 0.0, 0.0]
            } else {
                let channel = Channel::ALL[c];
                let v = stroke.material.values.get(channel);
                if channel.is_color() {
                    v.map(srgb_to_linear)
                } else {
                    v
                }
            };
            let params = ApplyParams {
                value: [value[0], value[1], value[2], 0.0],
                info: [stroke.opacity, erase, map.pool(c).components(), 0.0],
            };
            let offset = c * APPLY_STRIDE as usize;
            bytes[offset..offset + size_of::<ApplyParams>()]
                .copy_from_slice(bytemuck::bytes_of(&params));
        }
        queue.write_buffer(&self.apply_buffer, 0, &bytes);
        let instances: Vec<TileDraw> = draws.iter().map(|d| d.3).collect();
        let needed = (instances.len() * size_of::<TileDraw>()) as u64;
        if needed > self.draw_buffer.size() {
            self.draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("tile draws"),
                size: needed.next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.draw_buffer, 0, bytemuck::cast_slice(&instances));

        let groups = [PoolKind::Rgba, PoolKind::Rg].map(|kind| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("paint tiles"),
                layout: &self.tile_layout,
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
                        resource: self.draw_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(coverage),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(
                            self.snap[kind as usize].view(),
                        ),
                    },
                ],
            })
        });
        let mut start = 0;
        while start < draws.len() {
            let (mode, c, slot, _) = draws[start];
            let page = slot_place(slot).0;
            let end = start
                + draws[start..]
                    .iter()
                    .take_while(|d| d.0 == mode && d.1 == c && slot_place(d.2).0 == page)
                    .count();
            let kind = map.pool(c) as usize;
            let pipeline = match mode {
                Mode::Replace => &self.tile_replace[kind],
                Mode::Over => &self.tile_over[kind],
                Mode::Erase => &self.tile_erase[kind],
            };
            let mut pass = begin_pass(&mut encoder, self.live[kind].page_view(page), None);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &groups[kind], &[(c as u64 * APPLY_STRIDE) as u32]);
            pass.draw(0..6, start as u32..end as u32);
            drop(pass);
            start = end;
        }
        queue.submit([encoder.finish()]);
    }

    /// Undoes the strokes above `keep` by copying back the tiles they painted over.
    /// Returns false, having changed nothing, if a copy is missing.
    fn undo_strokes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: u64,
        map: &mut PaintMap,
        keep: usize,
        dirty: &mut Region,
    ) -> bool {
        let size = map.resolution;
        let matches =
            |s: &Snapshot, stroke: u64| s.paint == id && s.stroke == stroke && s.resolution == size;
        let available = |stroke: u64| {
            self.snapshot.as_ref().is_some_and(|s| matches(s, stroke))
                || self.pre_images.iter().any(|p| matches(p, stroke))
        };
        if !map.applied[keep..]
            .iter()
            .all(|&(stroke, _)| available(stroke))
        {
            return false;
        }
        let mut copies: Vec<Snapshot> = Vec::new();
        for &(stroke, _) in map.applied[keep..].iter().rev() {
            if let Some(s) = self.snapshot.take_if(|s| matches(s, stroke)) {
                copies.push(s);
            } else if let Some(i) = self.pre_images.iter().position(|p| matches(p, stroke))
                && let Some(p) = self.pre_images.remove(i)
            {
                copies.push(p);
            }
        }
        let mut needed = [0u32; 2];
        for s in &copies {
            for (kind, _) in s.copies() {
                needed[kind as usize] += 1;
            }
        }
        for kind in [PoolKind::Rgba, PoolKind::Rg] {
            self.live[kind as usize].reserve(device, queue, needed[kind as usize]);
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        // Topmost stroke first, so each tile ends up as it was before the lowest one.
        for snapshot in &copies {
            for (&(c, t), &before) in &snapshot.before {
                let kind = map.pool(c) as usize;
                let current = map.slots[c][t as usize];
                if before == EMPTY {
                    if current != EMPTY {
                        self.live[kind].release(current);
                        map.slots[c][t as usize] = EMPTY;
                    }
                } else {
                    let slot = if current == EMPTY {
                        let slot = self.live[kind].allocate(device, queue);
                        map.slots[c][t as usize] = slot;
                        slot
                    } else {
                        current
                    };
                    copy_tile(
                        &mut encoder,
                        self.snap[kind].texture(),
                        before,
                        self.live[kind].texture(),
                        slot,
                    );
                }
                map.slots_dirty = true;
                dirty.add(map.tile_rect(t));
            }
        }
        queue.submit([encoder.finish()]);
        for snapshot in &copies {
            self.release_copies(snapshot);
        }
        map.applied.truncate(keep);
        true
    }

    /// Starts tracking the tiles a new topmost stroke paints over. The previous snapshot is
    /// kept as a pre-image, within budget.
    fn start_snapshot(&mut self, paint: u64, stroke: u64, resolution: u32, gray: bool) {
        if let Some(old) = self.snapshot.take() {
            self.keep_pre_image(old);
        }
        self.snapshot = Some(Snapshot {
            paint,
            stroke,
            resolution,
            gray,
            before: HashMap::new(),
        });
    }

    fn copies_bytes(&self, snapshot: &Snapshot) -> u64 {
        snapshot
            .copies()
            .map(|(kind, _)| self.snap[kind as usize].bytes_per_tile())
            .sum()
    }

    /// Keeps a stroke's snapshot for undo. Snapshots of strokes that only painted empty
    /// tiles copy nothing, but are kept too: undoing them just frees those tiles.
    fn keep_pre_image(&mut self, snapshot: Snapshot) {
        let bytes = self.copies_bytes(&snapshot);
        if bytes > PRE_IMAGE_BUDGET {
            self.release_copies(&snapshot);
            return;
        }
        let mut used: u64 = self.pre_images.iter().map(|p| self.copies_bytes(p)).sum();
        while (used + bytes > PRE_IMAGE_BUDGET || self.pre_images.len() >= PRE_IMAGES_KEPT)
            && let Some(oldest) = self.pre_images.pop_front()
        {
            used -= self.copies_bytes(&oldest);
            self.release_copies(&oldest);
        }
        self.pre_images.push_back(snapshot);
    }

    /// Frees the snapshot pool slots of a snapshot or pre-image.
    fn release_copies(&mut self, snapshot: &Snapshot) {
        for (kind, slot) in snapshot.copies() {
            self.snap[kind as usize].release(slot);
        }
    }

    /// Drops the snapshot and pre-images of a paint layer, before it is painted again from
    /// scratch or goes away.
    fn forget(&mut self, paint: u64) {
        if let Some(snapshot) = self.snapshot.take_if(|s| s.paint == paint) {
            self.release_copies(&snapshot);
        }
        let (gone, kept): (Vec<Snapshot>, Vec<Snapshot>) = std::mem::take(&mut self.pre_images)
            .into_iter()
            .partition(|p| p.paint == paint);
        self.pre_images = kept.into();
        for pre in gone {
            self.release_copies(&pre);
        }
    }

    fn release_map(&mut self, map: &PaintMap) {
        for (c, slots) in map.slots.iter().enumerate() {
            let kind = map.pool(c) as usize;
            for &slot in slots.iter().filter(|&&s| s != EMPTY) {
                self.live[kind].release(slot);
            }
        }
    }

    fn clear_map(&mut self, map: &mut PaintMap) {
        self.release_map(map);
        for slots in &mut map.slots {
            slots.fill(EMPTY);
        }
        map.slots_dirty = true;
        map.applied.clear();
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
        size: u32,
        coverage: &wgpu::TextureView,
        stroke: &Stroke,
        dabs: &[Dab],
    ) -> Region {
        let tip = match &stroke.alpha {
            Alpha::Round => 0.0,
            Alpha::Square => 1.0,
            Alpha::Image(_) => 2.0,
        };
        let alpha = self.alpha_view(device, queue, &stroke.alpha);
        // The uploaded size, which is smaller than the image if it exceeded the device limit.
        let alpha_size = {
            let t = alpha.texture();
            t.width().max(t.height()) as f32
        };
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

        let mut changed = Region::default();
        let mut indices: Vec<u32> = Vec::new();
        let mut nearby: Vec<u32> = Vec::new();
        for chunk in dabs.chunks(MAX_DABS_PER_DRAW) {
            let (region, count) = match stroke.space {
                StrokeSpace::Surface => {
                    indices.clear();
                    let region = self.select_triangles(
                        model,
                        set,
                        stroke,
                        chunk,
                        size,
                        &mut nearby,
                        &mut indices,
                    );
                    (region, indices.len() as u32)
                }
                StrokeSpace::Uv => {
                    let mut region = Region::default();
                    for dab in chunk {
                        let (p, r) = (dab.position, dab.radius);
                        region.add(TexelRect::from_uv(
                            [p.x - r, p.y - r],
                            [p.x + r, p.y + r],
                            size,
                            1,
                        ));
                    }
                    (region, 3)
                }
            };
            if region.is_empty() || count == 0 {
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
                let mut pass = begin_pass(&mut encoder, coverage, None);
                region.bounds().scissor(&mut pass);
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
            changed.extend(&region);
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

    /// Collects the vertex indices of the triangles the dabs can touch. Returns the texels
    /// they cover.
    #[allow(clippy::too_many_arguments)]
    fn select_triangles(
        &mut self,
        model: &Model,
        set: usize,
        stroke: &Stroke,
        dabs: &[Dab],
        size: u32,
        nearby: &mut Vec<u32>,
        indices: &mut Vec<u32>,
    ) -> Region {
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
        let mut region = Region::default();
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
                let mut min = [f32::MAX; 2];
                let mut max = [f32::MIN; 2];
                for v in model.triangle_vertices(t) {
                    indices.push(v as u32);
                    let uv = model.mesh.uvs[v];
                    min = [min[0].min(uv.x), min[1].min(uv.y)];
                    max = [max[0].max(uv.x), max[1].max(uv.y)];
                }
                region.add(TexelRect::from_uv(min, max, size, 1));
            }
        }
        region
    }
}

/// The paint layers of a stack, with whether they are grayscale (masks, paint effects).
fn paint_roles(stack: &ink_core::LayerStack) -> Vec<(PaintRole, &[Arc<Stroke>])> {
    let mut roles = Vec::new();
    for layer in stack.iter() {
        if let Content::Paint(paint) = &layer.content {
            roles.push((
                PaintRole {
                    id: paint.id,
                    gray: false,
                },
                paint.strokes.as_slice(),
            ));
        }
        if let Some(mask) = &layer.mask {
            let effects = mask.effects.iter().filter_map(|e| match &e.kind {
                EffectKind::Paint(paint) => Some(paint),
                _ => None,
            });
            for paint in std::iter::once(&mask.paint).chain(effects) {
                roles.push((
                    PaintRole {
                        id: paint.id,
                        gray: true,
                    },
                    paint.strokes.as_slice(),
                ));
            }
        }
    }
    roles
}

/// The instance data drawing tile `tile` of a map into `slot`, over `before` (a snapshot
/// slot or [`EMPTY`]).
fn tile_draw(map: &PaintMap, tile: u32, slot: u32, before: u32) -> TileDraw {
    let (_, dest) = slot_place(slot);
    let origin = map.tile_rect(tile).min;
    let (page, from) = if before == EMPTY {
        (0, [0, 0])
    } else {
        slot_place(before)
    };
    TileDraw {
        dest: [dest[0], dest[1], origin[0], origin[1]],
        before: [u32::from(before != EMPTY), page, from[0], from[1]],
    }
}

/// Copies a whole tile from one pool slot to another.
fn copy_tile(
    encoder: &mut wgpu::CommandEncoder,
    from: &wgpu::Texture,
    from_slot: u32,
    to: &wgpu::Texture,
    to_slot: u32,
) {
    let place = |slot| {
        let (page, [x, y]) = slot_place(slot);
        wgpu::Origin3d { x, y, z: page }
    };
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            origin: place(from_slot),
            ..from.as_image_copy()
        },
        wgpu::TexelCopyTextureInfo {
            origin: place(to_slot),
            ..to.as_image_copy()
        },
        wgpu::Extent3d {
            width: TILE,
            height: TILE,
            depth_or_array_layers: 1,
        },
    );
}

/// The coverage texture in `slot`, made anew if its resolution differs.
fn coverage_view(
    slot: &mut Option<Coverage>,
    device: &wgpu::Device,
    resolution: u32,
) -> wgpu::TextureView {
    if slot.as_ref().is_none_or(|c| c.resolution != resolution) {
        let view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("stroke coverage"),
                size: wgpu::Extent3d {
                    width: resolution,
                    height: resolution,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: COVERAGE_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        *slot = Some(Coverage { resolution, view });
    }
    slot.as_ref().expect("made above").view.clone()
}

fn clear(device: &wgpu::Device, queue: &wgpu::Queue, view: &wgpu::TextureView) {
    let mut encoder = device.create_command_encoder(&Default::default());
    begin_pass(&mut encoder, view, Some(wgpu::Color::TRANSPARENT));
    queue.submit([encoder.finish()]);
}

/// A render pass on one color target, cleared to `clear` or keeping its contents.
pub(crate) fn begin_pass<'a>(
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

/// Uploads a grayscale alpha with a box-filtered mip chain. Levels larger than the device
/// allows are left out.
pub(crate) fn upload_alpha(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    let mut levels = vec![(width.max(1), height.max(1), pixels.to_vec())];
    while let Some((w, h, data)) = levels.last()
        && (*w > 1 || *h > 1)
    {
        let (w, h) = (*w, *h);
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
    let limit = device.limits().max_texture_dimension_2d;
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
        assert_eq!(size_of::<TileDraw>(), 32);
        assert!(size_of::<ApplyParams>() as u64 <= APPLY_STRIDE);
        for channel in Channel::ALL {
            let components = POOL_FORMATS[pool_kind(false, channel.index()) as usize].components();
            let wide = matches!(channel.kind(), ChannelKind::Color | ChannelKind::Normal);
            assert_eq!(components == 4, wide, "{channel:?}");
        }
        assert_eq!(pool_kind(true, 0), PoolKind::Rg);
    }
}
