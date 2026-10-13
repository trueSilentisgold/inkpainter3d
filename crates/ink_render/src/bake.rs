//! Baking mesh maps on the GPU: the triangles of a texture set are drawn in UV space to find
//! the triangle of every texel, and a compute shader casts rays from each texel's surface
//! point into bounding volume hierarchies of the model and of the high-poly meshes. Works
//! on any Vulkan GPU: no ray tracing hardware needed.

use egui_wgpu::wgpu;
use ink_core::bake::match_base;
use ink_core::{BakeSettings, Bvh, HighPoly, Mesh, MeshMap, Model, Texels, TextureImage};
use wgpu::util::DeviceExt;

/// Texels per side of the pieces the map is baked in.
const TILE: u32 = 256;
/// Floats per vertex in the vertex buffer: position, normal, uv, tangent (see `Vertex`).
const STRIDE: usize = 12;
const NONE: u32 = u32::MAX;

/// Must match `Params` in bake.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    tile: [i32; 4],
    config: [u32; 4],
    distances: [f32; 4],
    bounds_min: [f32; 4],
    bounds_size: [f32; 4],
    flags: [u32; 4],
    matching: [u32; 4],
    low_offsets: [u32; 4],
    high_offsets: [u32; 4],
}

/// Must match `RasterParams` in bake_raster.wgsl.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RasterParams {
    area: [f32; 4],
    total: [f32; 4],
}

/// The bake shader with its trace function written out for the low- and the high-poly data.
fn shader_source() -> String {
    let source = include_str!("bake.wgsl");
    let begin = source
        .find("// ---- TEMPLATE BEGIN ----")
        .expect("template");
    let end = source
        .find("// ---- TEMPLATE END ----")
        .expect("template end");
    let template = &source[begin..end];
    let instance = |name: &str, tri: &str, filter: &str| {
        template
            .replace("$FILTER", filter)
            .replace("$TRI", tri)
            .replace("$B", name)
    };
    let low = instance("low", "params.low_offsets.x", "");
    let high = instance(
        "high",
        "params.high_offsets.x",
        "if check_group {
                let g = bitcast<u32>(high_data[params.high_offsets.y + slot * 4u + 3u].z);
                if g != NONE && g != group {
                    continue;
                }
            }",
    );
    source
        .replace("$INSTANCES", &format!("{low}\n{high}"))
        .replace(template, "")
}

#[cfg(test)]
fn shader_for_tests() -> String {
    shader_source()
}

pub struct Baker {
    raster: wgpu::RenderPipeline,
    raster_layout: wgpu::BindGroupLayout,
    compute: wgpu::ComputePipeline,
    compute_layout: wgpu::BindGroupLayout,
}

/// What to bake.
pub struct BakeJob<'a> {
    pub model: &'a Model,
    pub set: usize,
    pub settings: &'a BakeSettings,
    /// The high-poly meshes, if any; maps that need them are skipped without.
    pub high: Option<&'a HighPoly>,
}

/// A BVH and its triangles, in the layout the shader reads: nodes (two vec4s each), then the
/// triangles' corners (three vec4s each), in the order of the leaves.
struct PackedBvh {
    data: Vec<[f32; 4]>,
    /// Index (in vec4s) of the first triangle.
    triangles: u32,
    /// The original index of the triangle in each slot.
    order: Vec<u32>,
}

fn pack_bvh(mesh: &Mesh, bvh: &Bvh) -> PackedBvh {
    let nodes = bvh.gpu_nodes();
    let mut data: Vec<[f32; 4]> = Vec::with_capacity(nodes.len() * 2 + mesh.triangle_count() * 3);
    for node in &nodes {
        data.push([node[0], node[1], node[2], node[3]]);
        data.push([node[4], node[5], node[6], node[7]]);
    }
    let triangles = data.len() as u32;
    for &t in bvh.order() {
        for k in 0..3 {
            let p = mesh.positions[mesh.indices[t as usize * 3 + k] as usize];
            data.push([p.x, p.y, p.z, 0.0]);
        }
    }
    PackedBvh {
        data,
        triangles,
        order: bvh.order().to_vec(),
    }
}

/// How a name's base is turned into a group number for [`ink_core::Matching::ByName`].
struct Groups {
    names: Vec<String>,
}

impl Groups {
    fn group(&mut self, name: &str) -> u32 {
        let Some(base) = match_base(name) else {
            return NONE;
        };
        match self.names.iter().position(|n| *n == base) {
            Some(i) => i as u32,
            None => {
                self.names.push(base);
                self.names.len() as u32 - 1
            }
        }
    }
}

fn storage(device: &wgpu::Device, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    // A buffer may not be empty.
    let padded = if bytes.is_empty() {
        &[0u8; 16][..]
    } else {
        bytes
    };
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: padded,
        usage: wgpu::BufferUsages::STORAGE,
    })
}

impl Baker {
    pub fn new(device: &wgpu::Device) -> Self {
        let compute_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bake"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let raster_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bake raster"),
            source: wgpu::ShaderSource::Wgsl(include_str!("bake_raster.wgsl").into()),
        });
        let entry = |binding, ty, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let buffer = |ty| wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let uniform = buffer(wgpu::BufferBindingType::Uniform);
        let read = buffer(wgpu::BufferBindingType::Storage { read_only: true });
        let compute_stage = wgpu::ShaderStages::COMPUTE;
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bake"),
            entries: &[
                entry(0, uniform, compute_stage),
                entry(1, read, compute_stage),
                entry(2, read, compute_stage),
                entry(3, read, compute_stage),
                entry(4, read, compute_stage),
                entry(
                    5,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    compute_stage,
                ),
                entry(
                    6,
                    wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    compute_stage,
                ),
            ],
        });
        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("bake"),
                bind_group_layouts: &[Some(&compute_layout)],
                immediate_size: 0,
            });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("bake"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_module,
            entry_point: Some("bake_main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let vertex = wgpu::ShaderStages::VERTEX;
        let raster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bake raster"),
            entries: &[
                entry(0, uniform, vertex),
                entry(1, read, vertex),
                entry(2, read, vertex),
                entry(3, read, vertex),
            ],
        });
        let raster_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("bake raster"),
                bind_group_layouts: &[Some(&raster_layout)],
                immediate_size: 0,
            });
        let raster = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bake raster"),
            layout: Some(&raster_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &raster_module,
                entry_point: Some("vs_triangles"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &raster_module,
                entry_point: Some("fs_triangles"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R32Uint,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            raster,
            raster_layout,
            compute,
            compute_layout,
        }
    }

    /// Bakes the maps of the job's texture set that the settings ask for. `progress` gets
    /// the fraction done and says whether to go on.
    pub fn bake(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        job: &BakeJob,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Vec<(MeshMap, TextureImage)>, String> {
        let model = job.model;
        let settings = job.settings;
        let set = &model.texture_sets[job.set];
        let res = set.resolution;
        let sub = settings.subsampling.clamp(1, 4);
        let maps: Vec<MeshMap> = MeshMap::ALL
            .into_iter()
            .filter(|m| settings.is_on(*m))
            .filter(|m| !m.needs_high_poly() || job.high.is_some_and(|h| !h.is_empty()))
            .collect();
        if maps.is_empty() {
            return Ok(Vec::new());
        }
        let parts: Vec<_> = model
            .parts
            .iter()
            .filter(|p| p.texture_set == job.set)
            .collect();
        if parts.is_empty() {
            return Err("The texture set has no triangles".to_owned());
        }

        // Geometry, as the shaders read it.
        let mesh = &model.mesh;
        let mut vertices: Vec<f32> = Vec::with_capacity(mesh.vertex_count() * STRIDE);
        for i in 0..mesh.vertex_count() {
            let (p, n, uv) = (mesh.positions[i], mesh.normals[i], mesh.uvs[i]);
            let t = mesh
                .tangents
                .get(i)
                .copied()
                .unwrap_or(glam::Vec4::new(1.0, 0.0, 0.0, 1.0));
            vertices.extend([p.x, p.y, p.z, n.x, n.y, n.z, uv.x, uv.y, t.x, t.y, t.z, t.w]);
        }
        let low = pack_bvh(mesh, model.bvh());
        let mut groups = Groups { names: Vec::new() };
        let mut indices = mesh.indices.clone();
        let triangle_count = mesh.triangle_count();
        let mut low_groups = vec![NONE; triangle_count];
        for part in &model.parts {
            let group = groups.group(&model.objects[part.object].name);
            for t in (part.indices.start / 3)..(part.indices.end / 3) {
                low_groups[t as usize] = group;
            }
        }
        indices.extend(&low_groups);

        let high_data: Vec<[f32; 4]>;
        let high_tris;
        let high_attr;
        match job.high.filter(|h| !h.is_empty()) {
            Some(high) => {
                let bvh = Bvh::new(&high.mesh);
                let packed = pack_bvh(&high.mesh, &bvh);
                let mut data = packed.data;
                high_tris = packed.triangles;
                high_attr = data.len() as u32;
                let mut high_groups = Groups {
                    names: groups.names.clone(),
                };
                let object_groups: Vec<u32> = high
                    .object_names
                    .iter()
                    .map(|n| high_groups.group(n))
                    .collect();
                for &t in &packed.order {
                    let t = t as usize;
                    for k in 0..3 {
                        let n = high.mesh.normals[high.mesh.indices[t * 3 + k] as usize];
                        data.push([n.x, n.y, n.z, 0.0]);
                    }
                    let object = high.triangle_object[t];
                    data.push([
                        f32::from_bits(object),
                        f32::from_bits(high.triangle_material[t]),
                        f32::from_bits(object_groups.get(object as usize).copied().unwrap_or(NONE)),
                        0.0,
                    ]);
                }
                high_data = data;
            }
            None => {
                high_data = Vec::new();
                high_tris = 0;
                high_attr = 0;
            }
        }

        let bounds = mesh.bounds();
        let size = bounds.max - bounds.min;
        let diagonal = size.length().max(1e-9);
        let epsilon = diagonal * 1e-4;
        let low_buffer = storage(device, "bake low", bytemuck::cast_slice(&low.data));
        let high_buffer = storage(device, "bake high", bytemuck::cast_slice(&high_data));
        let vertex_buffer = storage(device, "bake vertices", bytemuck::cast_slice(&vertices));
        let index_buffer = storage(device, "bake indices", bytemuck::cast_slice(&indices));
        let raster_indices: Vec<u32> = parts
            .iter()
            .flat_map(|p| p.indices.clone())
            .map(|i| mesh.indices[i as usize])
            .collect();
        // The raster pass numbers triangles by its own vertex order: map them back.
        let raster_triangles: Vec<u32> = parts
            .iter()
            .flat_map(|p| (p.indices.start / 3)..(p.indices.end / 3))
            .collect();
        let raster_index_buffer = storage(
            device,
            "bake raster indices",
            bytemuck::cast_slice(&raster_indices),
        );
        let raster_triangle_buffer = storage(
            device,
            "bake raster triangles",
            bytemuck::cast_slice(&raster_triangles),
        );

        let mut results = Vec::new();
        let tiles = {
            let per_side = res.div_ceil(TILE);
            per_side * per_side
        };
        let total_steps = (tiles as usize * maps.len()) as f32;
        let mut step = 0usize;
        for map in maps {
            let out = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("bake output"),
                size: wgpu::Extent3d {
                    width: res,
                    height: res,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let out_view = out.create_view(&Default::default());
            let (ao_rays, thickness_rays) =
                (settings.ao_rays.max(1), settings.thickness_rays.max(1));
            let rays = match map {
                MeshMap::AmbientOcclusion => ao_rays,
                MeshMap::Thickness => thickness_rays,
                MeshMap::Curvature => 16,
                _ => 0,
            };
            let ao_distance = settings.ao_distance.max(1e-4) * diagonal;
            let distance = match map {
                MeshMap::Thickness => settings.thickness_distance.max(1e-4) * diagonal,
                _ => ao_distance,
            };
            let mut y = 0;
            while y < res {
                let mut x = 0;
                while x < res {
                    let (w, h) = (TILE.min(res - x), TILE.min(res - y));
                    let params = Params {
                        tile: [x as i32, y as i32, w as i32, h as i32],
                        config: [
                            res,
                            sub,
                            MeshMap::ALL.iter().position(|m| *m == map).unwrap() as u32,
                            rays,
                        ],
                        distances: [
                            settings.front_distance.max(1e-5) * diagonal,
                            settings.rear_distance.max(1e-5) * diagonal,
                            distance,
                            settings.curvature_radius.max(1e-5) * diagonal,
                        ],
                        bounds_min: [bounds.min.x, bounds.min.y, bounds.min.z, 0.0],
                        bounds_size: [size.x, size.y, size.z, epsilon],
                        flags: [
                            settings.average_normals as u32,
                            settings.ignore_backfaces as u32,
                            // AO and thickness see the high-poly if there is one and the
                            // low-poly's own occlusion is off, or when it is the only one.
                            (job.high.is_some_and(|h| !h.is_empty())
                                && (!settings.ao_self_occlusion || !map_uses_low(map)))
                                as u32,
                            settings.id_source as u32,
                        ],
                        matching: [
                            (settings.matching == ink_core::Matching::ByName) as u32,
                            triangle_count as u32,
                            0,
                            0,
                        ],
                        low_offsets: [low.triangles, 0, 0, 0],
                        high_offsets: [high_tris, high_attr, 0, 0],
                    };
                    let tri_map = self.triangle_map(
                        device,
                        queue,
                        &vertex_buffer,
                        &raster_index_buffer,
                        &raster_triangle_buffer,
                        raster_indices.len() as u32,
                        (x, y, w, h),
                        res,
                        sub,
                    );
                    let params_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("bake params"),
                            contents: bytemuck::bytes_of(&params),
                            usage: wgpu::BufferUsages::UNIFORM,
                        });
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("bake"),
                        layout: &self.compute_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: params_buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: low_buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: high_buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: vertex_buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 4,
                                resource: index_buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 5,
                                resource: wgpu::BindingResource::TextureView(&tri_map),
                            },
                            wgpu::BindGroupEntry {
                                binding: 6,
                                resource: wgpu::BindingResource::TextureView(&out_view),
                            },
                        ],
                    });
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = encoder.begin_compute_pass(&Default::default());
                        pass.set_pipeline(&self.compute);
                        pass.set_bind_group(0, &group, &[]);
                        pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
                    }
                    queue.submit([encoder.finish()]);
                    // Wait for each tile: the GPU stays usable for the UI in between.
                    device
                        .poll(wgpu::PollType::wait_indefinitely())
                        .map_err(|e| e.to_string())?;
                    step += 1;
                    if !progress(step as f32 / total_steps) {
                        return Err("Cancelled".to_owned());
                    }
                    x += TILE;
                }
                y += TILE;
            }
            let image = read_map(device, queue, &out, res, map, settings.dilation)?;
            results.push((map, image));
        }
        Ok(results)
    }

    /// The triangle (its index in the mesh) under every sub-texel of a tile, or `u32::MAX`.
    #[allow(clippy::too_many_arguments)]
    fn triangle_map(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        triangles: &wgpu::Buffer,
        index_count: u32,
        tile: (u32, u32, u32, u32),
        res: u32,
        sub: u32,
    ) -> wgpu::TextureView {
        let (x, y, w, h) = tile;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bake triangles"),
            size: wgpu::Extent3d {
                width: w * sub,
                height: h * sub,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let params = RasterParams {
            area: [
                (x * sub) as f32,
                (y * sub) as f32,
                (w * sub) as f32,
                (h * sub) as f32,
            ],
            total: [(res * sub) as f32, 0.0, 0.0, 0.0],
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bake raster params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bake raster"),
            layout: &self.raster_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: vertices.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: indices.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: triangles.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = crate::paint::begin_pass(
                &mut encoder,
                &view,
                Some(wgpu::Color {
                    r: u32::MAX as f64,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                }),
            );
            pass.set_pipeline(&self.raster);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..index_count, 0..1);
        }
        queue.submit([encoder.finish()]);
        view
    }
}

/// Whether the map looks at the model's own triangles when there is no high-poly to see.
fn map_uses_low(map: MeshMap) -> bool {
    matches!(
        map,
        MeshMap::AmbientOcclusion | MeshMap::Thickness | MeshMap::Curvature
    )
}

/// Reads the baked texture, pads it past the islands and turns it into an image.
fn read_map(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    res: u32,
    map: MeshMap,
    dilation: u32,
) -> Result<TextureImage, String> {
    let row = res * 8;
    let bytes_per_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bake readback"),
        size: bytes_per_row as u64 * res as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(res),
            },
        },
        wgpu::Extent3d {
            width: res,
            height: res,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    let mut texels = vec![[0f32; 4]; (res * res) as usize];
    {
        let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
        for y in 0..res as usize {
            for x in 0..res as usize {
                let at = y * bytes_per_row as usize + x * 8;
                texels[y * res as usize + x] = std::array::from_fn(|c| {
                    half::f16::from_le_bytes([data[at + 2 * c], data[at + 2 * c + 1]]).to_f32()
                });
            }
        }
    }
    buffer.unmap();
    dilate(&mut texels, res, dilation, map.background());
    let rgba: Vec<u16> = texels
        .iter()
        .flat_map(|t| {
            [
                (t[0].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                (t[1].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                (t[2].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16,
                u16::MAX,
            ]
        })
        .collect();
    Ok(TextureImage::new(
        map.label().to_owned(),
        res,
        res,
        Texels::Rgba16(rgba),
    ))
}

/// Fills the texels outside the islands (alpha 0) with the value of the nearest covered
/// texel, up to `distance` texels away; the rest get `background`. Afterwards alpha is 1.
fn dilate(texels: &mut [[f32; 4]], res: u32, distance: u32, background: [f32; 3]) {
    let res = res as usize;
    let mut covered: Vec<bool> = texels.iter().map(|t| t[3] > 0.0).collect();
    for _ in 0..distance {
        let mut changed = false;
        let before = covered.clone();
        for y in 0..res {
            for x in 0..res {
                if before[y * res + x] {
                    continue;
                }
                let mut sum = [0.0f32; 3];
                let mut n = 0.0;
                for (dx, dy) in [
                    (-1i32, 0i32),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, -1),
                    (-1, 1),
                    (1, 1),
                ] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= res as i32 || ny >= res as i32 {
                        continue;
                    }
                    let i = ny as usize * res + nx as usize;
                    if before[i] {
                        for c in 0..3 {
                            sum[c] += texels[i][c];
                        }
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    texels[y * res + x] = [sum[0] / n, sum[1] / n, sum[2] / n, 1.0];
                    covered[y * res + x] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    for (t, covered) in texels.iter_mut().zip(&covered) {
        if !covered {
            *t = [background[0], background[1], background[2], 1.0];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_the_shaders() {
        assert_eq!(size_of::<Params>(), 9 * 16);
        assert_eq!(size_of::<RasterParams>(), 32);
    }

    #[test]
    fn the_shader_is_valid() {
        let source = shader_for_tests();
        let module = naga::front::wgsl::parse_str(&source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{e:?}"));
        let raster = include_str!("bake_raster.wgsl");
        let module = naga::front::wgsl::parse_str(raster)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(raster)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn dilation_reaches_as_far_as_asked() {
        let mut texels = vec![[0.0; 4]; 16];
        texels[0] = [1.0, 0.5, 0.25, 1.0];
        dilate(&mut texels, 4, 1, [0.0; 3]);
        assert_eq!(texels[1][..3], [1.0, 0.5, 0.25]);
        assert_eq!(texels[5][..3], [1.0, 0.5, 0.25]);
        assert_eq!(
            texels[2][..3],
            [0.0, 0.0, 0.0],
            "one step reaches one texel"
        );
        assert!(texels.iter().all(|t| t[3] == 1.0));
    }
}
