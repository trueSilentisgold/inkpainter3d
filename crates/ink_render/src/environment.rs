//! Image based lighting: turns an equirectangular HDR panorama into the textures and
//! coefficients the viewport shader lights with.

use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::Vec3;
use half::f16;
use std::f32::consts::PI;

/// Face size of the prefiltered specular cube map, mip 0.
const SPECULAR_SIZE: u32 = 256;
/// Mip levels from mirror-like (0) to fully rough (last): 256 down to 8 pixels.
pub const SPECULAR_MIPS: u32 = 6;
const PREFILTER_SAMPLES: u32 = 128;
const BRDF_LUT_SIZE: u32 = 128;
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Largest value a 16-bit float can hold; brighter pixels (e.g. the sun) are clamped.
const F16_MAX: f32 = 65000.0;

/// GPU textures and coefficients for lighting with one environment.
pub struct Environment {
    /// The panorama itself with a full mip chain, for the background.
    pub panorama: wgpu::TextureView,
    /// GGX-prefiltered cube map; the mip level selects the roughness.
    pub specular: wgpu::TextureView,
    /// Diffuse irradiance as 9 spherical harmonics coefficients (rgb), already convolved
    /// with the cosine lobe, in the basis order used by `irradiance` in viewport.wgsl.
    pub irradiance_sh: [[f32; 4]; 9],
    /// Direction towards the brightest part of the environment, used as the toon light.
    pub dominant_direction: Vec3,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PrefilterParams {
    roughness: f32,
    size: f32,
    sample_count: u32,
    _pad: u32,
    source_size: [f32; 2],
    _pad2: [f32; 2],
}

/// Compute pipelines for the environment precomputation.
pub struct IblBaker {
    prefilter: wgpu::ComputePipeline,
    prefilter_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl IblBaker {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prefilter shader"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("ibl_common.wgsl"),
                    include_str!("prefilter.wgsl")
                )
                .into(),
            ),
        });
        let prefilter_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prefilter layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: HDR_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("prefilter pipeline layout"),
            bind_group_layouts: &[Some(&prefilter_layout)],
            immediate_size: 0,
        });
        let prefilter = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("prefilter"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("prefilter"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            prefilter,
            prefilter_layout,
            sampler: panorama_sampler(device),
        }
    }

    /// Uploads the panorama (linear RGBA, rows top to bottom) and precomputes the lighting.
    pub fn bake(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        pixels: &[[f32; 4]],
    ) -> Environment {
        assert_eq!(pixels.len(), (width * height) as usize);
        let levels = mip_chain(width, height, pixels);

        let panorama = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("environment panorama"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (mip, level) in levels.iter().enumerate() {
            let data: Vec<f16> = level
                .pixels
                .iter()
                .flat_map(|p| p.map(|c| f16::from_f32(c.min(F16_MAX))))
                .collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &panorama,
                    mip_level: mip as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&data),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(level.width * 8),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: level.width,
                    height: level.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let panorama_view = panorama.create_view(&Default::default());

        let specular = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("environment specular"),
            size: wgpu::Extent3d {
                width: SPECULAR_SIZE,
                height: SPECULAR_SIZE,
                depth_or_array_layers: 6,
            },
            mip_level_count: SPECULAR_MIPS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("prefilter environment"),
        });
        // Keep the per-mip resources alive until the commands are submitted.
        let mut bind_groups = Vec::new();
        for mip in 0..SPECULAR_MIPS {
            let size = SPECULAR_SIZE >> mip;
            let params = PrefilterParams {
                roughness: mip as f32 / (SPECULAR_MIPS - 1) as f32,
                size: size as f32,
                sample_count: PREFILTER_SAMPLES,
                _pad: 0,
                source_size: [width as f32, height as f32],
                _pad2: [0.0; 2],
            };
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("prefilter params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let target = specular.create_view(&wgpu::TextureViewDescriptor {
                label: Some("prefilter target"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..Default::default()
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("prefilter bind group"),
                layout: &self.prefilter_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&panorama_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&target),
                    },
                ],
            });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("prefilter pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.prefilter);
                pass.set_bind_group(0, &bind_group, &[]);
                let groups = size.div_ceil(8);
                pass.dispatch_workgroups(groups, groups, 6);
            }
            bind_groups.push(bind_group);
        }
        queue.submit([encoder.finish()]);

        let sh_level = levels
            .iter()
            .find(|l| l.width <= 128)
            .unwrap_or(levels.last().expect("at least one level"));
        let irradiance_sh = irradiance_sh(sh_level);
        Environment {
            panorama: panorama_view,
            specular: specular.create_view(&wgpu::TextureViewDescriptor {
                label: Some("environment specular cube"),
                dimension: Some(wgpu::TextureViewDimension::Cube),
                ..Default::default()
            }),
            dominant_direction: dominant_direction(&irradiance_sh),
            irradiance_sh,
        }
    }
}

/// Repeats horizontally (the panorama wraps around) and clamps vertically.
pub fn panorama_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("panorama sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    })
}

/// Creates the split-sum BRDF lookup table used for specular image based lighting.
pub fn create_brdf_lut(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("brdf lut shader"),
        source: wgpu::ShaderSource::Wgsl(
            concat!(
                include_str!("ibl_common.wgsl"),
                include_str!("brdf_lut.wgsl")
            )
            .into(),
        ),
    });
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("brdf lut"),
        size: wgpu::Extent3d {
            width: BRDF_LUT_SIZE,
            height: BRDF_LUT_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("brdf lut"),
        layout: None,
        module: &module,
        entry_point: Some("brdf_lut"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("brdf lut bind group"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("brdf lut"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("brdf lut pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        let groups = BRDF_LUT_SIZE.div_ceil(8);
        pass.dispatch_workgroups(groups, groups, 1);
    }
    queue.submit([encoder.finish()]);
    view
}

struct Level {
    width: u32,
    height: u32,
    pixels: Vec<[f32; 4]>,
}

/// Box-filtered mip chain down to 1x1, level 0 first.
fn mip_chain(width: u32, height: u32, pixels: &[[f32; 4]]) -> Vec<Level> {
    let mut levels = vec![Level {
        width,
        height,
        pixels: pixels.to_vec(),
    }];
    loop {
        let prev = levels.last().expect("level 0 exists");
        if prev.width == 1 && prev.height == 1 {
            return levels;
        }
        let (w, h) = ((prev.width / 2).max(1), (prev.height / 2).max(1));
        let mut next = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(prev.width - 1);
                    let sy = (y * 2 + dy).min(prev.height - 1);
                    let p = prev.pixels[(sy * prev.width + sx) as usize];
                    for c in 0..4 {
                        sum[c] += p[c] * 0.25;
                    }
                }
                next.push(sum);
            }
        }
        levels.push(Level {
            width: w,
            height: h,
            pixels: next,
        });
    }
}

/// Direction for a panorama coordinate; the inverse of `equirect_uv` in the shaders.
fn equirect_direction(u: f32, v: f32) -> Vec3 {
    let phi = (u - 0.5) * 2.0 * PI;
    let theta = v * PI;
    Vec3::new(
        theta.sin() * phi.sin(),
        theta.cos(),
        -theta.sin() * phi.cos(),
    )
}

/// Real spherical harmonics basis up to band 2, in the order used by the shader.
fn sh_basis(d: Vec3) -> [f32; 9] {
    let (x, y, z) = (d.x, d.y, d.z);
    [
        0.282095,
        0.488603 * y,
        0.488603 * z,
        0.488603 * x,
        1.092548 * x * y,
        1.092548 * y * z,
        0.315392 * (3.0 * z * z - 1.0),
        1.092548 * x * z,
        0.546274 * (x * x - y * y),
    ]
}

/// Projects the panorama onto spherical harmonics and convolves with the cosine lobe
/// (Ramamoorthi and Hanrahan), so evaluating the basis gives the irradiance.
fn irradiance_sh(level: &Level) -> [[f32; 4]; 9] {
    let mut sh = [[0.0f32; 4]; 9];
    let d_phi = 2.0 * PI / level.width as f32;
    let d_theta = PI / level.height as f32;
    for y in 0..level.height {
        let v = (y as f32 + 0.5) / level.height as f32;
        let solid_angle = d_phi * d_theta * (v * PI).sin();
        for x in 0..level.width {
            let u = (x as f32 + 0.5) / level.width as f32;
            let basis = sh_basis(equirect_direction(u, v));
            let p = level.pixels[(y * level.width + x) as usize];
            for (coefficient, b) in sh.iter_mut().zip(basis) {
                for c in 0..3 {
                    coefficient[c] += p[c] * b * solid_angle;
                }
            }
        }
    }
    let bands = [PI, 2.0 * PI / 3.0, PI / 4.0];
    for (i, coefficient) in sh.iter_mut().enumerate() {
        let band = match i {
            0 => 0,
            1..=3 => 1,
            _ => 2,
        };
        for c in coefficient.iter_mut().take(3) {
            *c *= bands[band];
        }
    }
    sh
}

/// Direction of the strongest linear (band 1) component of the lighting.
fn dominant_direction(sh: &[[f32; 4]; 9]) -> Vec3 {
    let luminance = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    // Band 1 is ordered y, z, x.
    let d = Vec3::new(luminance(sh[3]), luminance(sh[1]), luminance(sh[2]));
    d.try_normalize()
        .unwrap_or(Vec3::new(0.5, 0.7, 0.5).normalize())
}

/// Evaluates the irradiance like the shader does; for tests.
#[cfg(test)]
fn evaluate_sh(sh: &[[f32; 4]; 9], n: Vec3) -> [f32; 3] {
    let basis = sh_basis(n);
    let mut out = [0.0; 3];
    for (coefficient, b) in sh.iter().zip(basis) {
        for c in 0..3 {
            out[c] += coefficient[c] * b;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panorama(width: u32, height: u32, f: impl Fn(Vec3) -> f32) -> Level {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let d = equirect_direction(
                    (x as f32 + 0.5) / width as f32,
                    (y as f32 + 0.5) / height as f32,
                );
                let value = f(d);
                pixels.push([value, value, value, 1.0]);
            }
        }
        Level {
            width,
            height,
            pixels,
        }
    }

    #[test]
    fn uniform_environment_gives_pi_irradiance() {
        // A white furnace of radiance 1 gives irradiance π on every surface.
        let sh = irradiance_sh(&panorama(128, 64, |_| 1.0));
        for n in [
            Vec3::X,
            Vec3::Y,
            Vec3::NEG_Z,
            Vec3::new(1.0, 1.0, 1.0).normalize(),
        ] {
            let e = evaluate_sh(&sh, n)[0];
            assert!((e - PI).abs() < 0.02, "{n}: {e}");
        }
    }

    #[test]
    fn bright_sky_lights_from_above() {
        let sh = irradiance_sh(&panorama(128, 64, |d| if d.y > 0.0 { 1.0 } else { 0.0 }));
        let up = evaluate_sh(&sh, Vec3::Y)[0];
        let down = evaluate_sh(&sh, Vec3::NEG_Y)[0];
        // Exact: π up, 0 down; order 2 SH ringing stays small.
        assert!((up - PI).abs() < 0.2, "{up}");
        assert!(down.abs() < 0.2, "{down}");
        assert!(dominant_direction(&sh).dot(Vec3::Y) > 0.99);
    }

    #[test]
    fn mip_chain_ends_at_one_pixel_and_preserves_mean() {
        let level = panorama(64, 32, |d| d.y.max(0.0));
        let mean = |l: &Level| l.pixels.iter().map(|p| p[0]).sum::<f32>() / l.pixels.len() as f32;
        let levels = mip_chain(level.width, level.height, &level.pixels);
        let last = levels.last().unwrap();
        assert_eq!((last.width, last.height), (1, 1));
        assert_eq!(levels.len(), 7);
        assert!((mean(&levels[0]) - mean(last)).abs() < 1e-4);
    }

    #[test]
    fn equirect_direction_round_trips() {
        // Same formula as equirect_uv in the shaders.
        for (u, v) in [(0.1, 0.2), (0.5, 0.5), (0.9, 0.7)] {
            let d = equirect_direction(u, v);
            let back_u = d.x.atan2(-d.z) / (2.0 * PI) + 0.5;
            let back_v = d.y.clamp(-1.0, 1.0).acos() / PI;
            assert!((back_u - u).abs() < 1e-5 && (back_v - v).abs() < 1e-5);
        }
    }
}
