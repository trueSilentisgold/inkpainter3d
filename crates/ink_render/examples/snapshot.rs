//! Renders a model to a PNG without opening a window, e.g. for documentation or to check
//! the renderer on a machine without a display:
//!
//! ```sh
//! cargo run -p ink_render --example snapshot -- model.fbx out.png [options]
//! ```
//!
//! `model.fbx` may also be `sphere` or `cube`. Options: `display=normal`, `yaw=35`, `toon`,
//! `env=file.hdr`.
//!
//! Layer stack of the painted texture set, to check the compositor: `color=0.9,0.6,0.2`
//! (sRGB), `rough=0.2`, `metal=1` (a fill layer at the bottom with these values);
//! `fill=0.2,0.5,0.9` (a fill layer on top with this base color), `image=file.png` (its base
//! color image), `normalmap=file.png` (its normal image; `opengl` if it is not DirectX),
//! `heightmap=file.png` (its height image), `triplanar` and `scale=4` (its projection);
//! `mask` (the fill gets a black mask and the strokes paint into it), `levels` (an inverting
//! levels effect in that mask); `blend=multiply` and `opacity=0.5` (blending of the top
//! layer's color, roughness and metallic); `folder` (puts the top layer in a folder),
//! `passthrough` (that folder passes through); `height` (strokes paint only height 1).
//!
//! Brush strokes through the middle of the view, to check the paint pipeline: `paint`
//! (horizontal, red and shiny), `erase` (vertical eraser stroke), `uv` (a diagonal stroke
//! in UV space), `size=8`, `hardness=0.8`, `tip=round|square`, `res=1024` (texture size),
//! `live=10` (paints in chunks of 10 dabs like the app does), `undo` or `undo=2` (removes
//! the last strokes again), `nocull` (paints surfaces facing away too), `nodepth` (paints hidden
//! surfaces too), `set=Name` (texture set to paint; default: the one in the middle),
//! `only=Name` (shows only this texture set after painting). `2d` renders the 2D (UV) view of the painted texture set instead.

use anyhow::{Context, Result};
use glam::Vec2;
use ink_core::{
    Alpha, BlendMode, Brush, BrushMaterial, Channel, ChannelSet, Content, Effect, EffectKind,
    Layer, LayerKind, Levels, Mask, Mesh, Model, ProjectionMode, Stroke, StrokePath, StrokeSpace,
    Tool, surface_dab, uv_dab,
};
use ink_render::{
    DisplayMode, Highlight, OrbitCamera, ShadingMode, UvCamera, UvView, ViewportRenderer,
    ViewportSettings, wgpu,
};
use std::path::Path;
use std::sync::Arc;

const SIZE: [u32; 2] = [1280, 800];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(model_path), Some(out_path)) = (args.get(1), args.get(2)) else {
        anyhow::bail!("usage: snapshot <model|sphere|cube> <out.png> [options]");
    };
    let option = |key: &str| {
        args[3..]
            .iter()
            .find_map(|a| a.strip_prefix(key)?.strip_prefix('='))
    };
    let display = match option("display") {
        Some(name) => *DisplayMode::ALL
            .iter()
            .find(|m| m.label().replace(' ', "").eq_ignore_ascii_case(name))
            .with_context(|| format!("unknown display mode {name}"))?,
        None => DisplayMode::Material,
    };
    let yaw: f32 = option("yaw").map_or(Ok(35.0), str::parse)?;
    let float = |key: &str| option(key).map(str::parse::<f32>).transpose();

    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    eprintln!("GPU: {}", adapter.get_info().name);
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;

    let mut renderer = ViewportRenderer::new(&device, &queue);
    let env = match option("env") {
        Some(path) => ink_io::load_environment(Path::new(path))?,
        None => ink_io::default_environment(),
    };
    renderer.set_environment(&device, &queue, env.width, env.height, &env.pixels);
    let mut model = match model_path.as_str() {
        "sphere" => Model::from_mesh(Mesh::uv_sphere(1.0, 96, 48)),
        "cube" => Model::from_mesh(Mesh::cube(1.5)),
        path => ink_io::load_model(Path::new(path))?,
    };
    if let Some(res) = option("res") {
        let res: u32 = res.parse()?;
        for set in &mut model.texture_sets {
            set.resolution = res;
        }
    }
    renderer.set_model(&device, &queue, &model);

    let mut camera = OrbitCamera::default();
    camera.frame(&model.bounds());
    camera.yaw = yaw.to_radians();
    camera.pitch = 0.15;

    let flag = |name: &str| args[3..].iter().any(|a| a == name);
    let brush = Brush {
        size: float("size")?.unwrap_or(8.0),
        hardness: float("hardness")?.unwrap_or(0.8),
        backface_culling: !flag("nocull"),
        alpha: match option("tip") {
            Some("square") => Alpha::Square,
            _ => Alpha::Round,
        },
        ..Default::default()
    };
    let live: usize = option("live").map_or(Ok(usize::MAX), str::parse)?;
    let view = Vec2::new(SIZE[0] as f32, SIZE[1] as f32);
    let mut strokes = Vec::new();
    if flag("paint") {
        let mut material = BrushMaterial::default();
        material.values.base_color = [0.9, 0.05, 0.03];
        material.values.roughness = 0.15;
        material.values.metallic = 1.0;
        strokes.push((
            Tool::Paint,
            StrokeSpace::Surface,
            material,
            view * Vec2::new(0.2, 0.5),
            view * Vec2::new(0.8, 0.5),
        ));
    }
    if flag("erase") {
        strokes.push((
            Tool::Erase,
            StrokeSpace::Surface,
            BrushMaterial::default(),
            view * Vec2::new(0.5, 0.25),
            view * Vec2::new(0.5, 0.75),
        ));
    }
    if flag("uv") {
        let mut material = BrushMaterial {
            channels: ChannelSet::NONE,
            ..Default::default()
        };
        material.channels.set(Channel::BaseColor, true);
        material.values.base_color = [0.1, 0.4, 1.0];
        strokes.push((
            Tool::Paint,
            StrokeSpace::Uv,
            material,
            Vec2::splat(0.1),
            Vec2::splat(0.9),
        ));
    }
    let center = {
        let (origin, dir) = camera.ray(view * 0.5, view);
        model.pick(origin, dir)
    };
    let set = match option("set") {
        Some(name) => model
            .texture_sets
            .iter()
            .position(|s| s.name.starts_with(name))
            .with_context(|| format!("no texture set {name}"))?,
        None => center.map_or(0, |hit| model.parts[hit.part].texture_set),
    };
    let target = build_stack(&mut model, set, &option, &flag)?;
    for layer in model.texture_sets[set].stack.iter() {
        let b = layer.blending.get(Channel::BaseColor);
        eprintln!(
            "Layer {} ({:?}): {} {:.0}%{}",
            layer.name,
            layer.kind(),
            b.mode.label(),
            b.opacity * 100.0,
            if layer.mask.is_some() { ", masked" } else { "" }
        );
    }
    if flag("height") {
        for (_, _, material, _, _) in &mut strokes {
            material.channels = ChannelSet::NONE;
            material.channels.set(Channel::Height, true);
            material.values.height = 1.0;
        }
    }
    let model_radius = model.bounds().radius();
    for (tool, space, material, from, to) in strokes {
        let project = |p: Vec2, _pressure: f32| match space {
            StrokeSpace::Surface => {
                let (origin, dir) = camera.ray(p, view);
                let hit = model.pick(origin, dir)?;
                let radius = brush.world_radius(model_radius, camera.view_height_at(hit.position));
                Some(surface_dab(
                    &hit,
                    dir,
                    camera.up(),
                    brush.angle,
                    radius,
                    brush.flow,
                ))
            }
            StrokeSpace::Uv => Some(uv_dab(p, brush.angle, brush.size / 200.0, brush.flow)),
        };
        let mut dabs = Vec::new();
        let mut path = StrokePath::start(from, 1.0, brush.spacing, 0.0, project, &mut dabs);
        if space == StrokeSpace::Uv {
            path.screen_step = 0.002;
        }
        path.move_to(to, 1.0, project, &mut dabs);
        eprintln!(
            "{tool:?} stroke on {}: {} dabs",
            model.texture_sets[set].name,
            dabs.len()
        );
        let stroke = Stroke {
            id: model.next_id(),
            tool,
            space,
            alpha: brush.alpha.clone(),
            hardness: brush.hardness,
            opacity: brush.opacity,
            backface_cos: brush.backface_cos(),
            material,
            camera: (space == StrokeSpace::Surface && !flag("nodepth"))
                .then(|| camera.stroke_camera(view.x / view.y)),
            hidden_objects: Vec::new(),
            hidden_sets: Vec::new(),
            dabs: Vec::new(),
        };
        let (stroke_id, paint) = (stroke.id, target);
        let layer = model.texture_sets[set].stack.paint_mut(paint).unwrap();
        layer.strokes.push(std::sync::Arc::new(stroke));
        for chunk in dabs.chunks(live) {
            let layer = model.texture_sets[set].stack.paint_mut(paint).unwrap();
            layer.extend_stroke(stroke_id, chunk.iter().copied());
            renderer.sync_texture_sets(&device, &queue, &model, true);
        }
    }
    if let Some(name) = option("only") {
        for set in &mut model.texture_sets {
            set.visible = set.name.starts_with(name);
        }
    }
    let undo: usize = match option("undo") {
        Some(n) => n.parse()?,
        None => flag("undo") as usize,
    };
    for _ in 0..undo {
        model.texture_sets[set]
            .stack
            .paint_mut(target)
            .unwrap()
            .strokes
            .pop();
    }
    if undo > 0 {
        eprintln!("Undid {undo} strokes");
    }
    let started = std::time::Instant::now();
    renderer.sync_texture_sets(&device, &queue, &model, false);
    // Layer stack changes are composited a few chunks per sync.
    while renderer.is_compositing() {
        renderer.sync_texture_sets(&device, &queue, &model, false);
    }
    device.poll(wgpu::PollType::wait_indefinitely())?;
    eprintln!("Final sync took {:?}", started.elapsed());
    let settings = ViewportSettings {
        display,
        shading: if args[3..].iter().any(|a| a == "toon") {
            ShadingMode::Toon
        } else {
            ShadingMode::Pbr
        },
        background_opacity: 0.6,
        ..Default::default()
    };
    let mut uv_view = UvView::new(&device, &renderer);
    let texture = if flag("2d") {
        uv_view.set_model(&device, &model);
        uv_view.resize(&device, SIZE);
        uv_view.render(
            &device,
            &queue,
            &renderer,
            &model,
            set,
            &UvCamera::default(),
            display,
            true,
        );
        uv_view.output_texture().context("no output")?
    } else {
        renderer.resize(&device, SIZE);
        renderer.render(
            &device,
            &queue,
            &camera,
            &settings,
            &model,
            Highlight::default(),
        );
        renderer.output_texture().context("no output")?
    };
    let row = (SIZE[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (row * SIZE[1]) as u64,
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
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: SIZE[0],
            height: SIZE[1],
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely())?;

    let data = buffer.slice(..).get_mapped_range()?;
    let mut pixels = Vec::with_capacity((SIZE[0] * SIZE[1] * 4) as usize);
    for y in 0..SIZE[1] as usize {
        let start = y * row as usize;
        pixels.extend_from_slice(&data[start..start + SIZE[0] as usize * 4]);
    }
    image::RgbaImage::from_raw(SIZE[0], SIZE[1], pixels)
        .context("image size")?
        .save(out_path)?;
    eprintln!("Wrote {out_path}");
    Ok(())
}

/// Builds the texture set's layer stack from the options. Returns the paint layer the
/// strokes go into.
fn build_stack<'a>(
    model: &mut Model,
    set: usize,
    option: &impl Fn(&str) -> Option<&'a str>,
    flag: &impl Fn(&str) -> bool,
) -> Result<u64> {
    let floats = |key: &str| -> Result<Option<Vec<f32>>> {
        option(key)
            .map(|v| {
                v.split(',')
                    .map(str::parse)
                    .collect::<Result<Vec<f32>, _>>()
            })
            .transpose()
            .map_err(Into::into)
    };
    let Model {
        ids, texture_sets, ..
    } = model;
    let stack = &mut texture_sets[set].stack;
    let mut target = match &stack.layers[0].content {
        Content::Paint(p) => p.id,
        _ => unreachable!("new stacks hold a paint layer"),
    };

    let (color, rough, metal) = (floats("color")?, floats("rough")?, floats("metal")?);
    if color.is_some() || rough.is_some() || metal.is_some() {
        let mut base = Layer::new(LayerKind::Fill, "Base".into(), ids);
        if let Content::Fill(fill) = &mut base.content {
            fill.channels = ChannelSet::NONE;
            if let Some(c) = color {
                fill.channels.set(Channel::BaseColor, true);
                fill.values.base_color = [c[0], c[1], c[2]];
            }
            if let Some(r) = rough {
                fill.channels.set(Channel::Roughness, true);
                fill.values.roughness = r[0];
            }
            if let Some(m) = metal {
                fill.channels.set(Channel::Metallic, true);
                fill.values.metallic = m[0];
            }
        }
        stack.layers.insert(0, base);
    }

    let mut top = stack.layers.last().map(|l| l.id).expect("a layer");
    if let Some(c) = floats("fill")? {
        let mut layer = Layer::new(LayerKind::Fill, "Fill layer 1".into(), ids);
        if let Content::Fill(fill) = &mut layer.content {
            fill.values.base_color = [c[0], c[1], c[2]];
            let load = |key: &str| -> Result<Option<Arc<ink_core::TextureImage>>> {
                option(key)
                    .map(|path| ink_io::load_texture(Path::new(path)).map(Arc::new))
                    .transpose()
            };
            if let Some(image) = load("image")? {
                fill.images[Channel::BaseColor.index()] = Some(image);
            }
            if let Some(image) = load("normalmap")? {
                fill.channels.set(Channel::Normal, true);
                fill.images[Channel::Normal.index()] = Some(image);
                fill.normal_directx = !flag("opengl");
            }
            if let Some(image) = load("heightmap")? {
                fill.channels.set(Channel::Height, true);
                fill.images[Channel::Height.index()] = Some(image);
            }
            if flag("triplanar") {
                fill.projection.mode = ProjectionMode::TriPlanar;
            }
            if let Some(scale) = floats("scale")? {
                fill.projection.scale = [scale[0], scale[0]];
            }
        }
        if flag("mask") {
            let mut mask = Mask::new(0.0, ids.next());
            target = mask.paint.id;
            if flag("levels") {
                let levels = Levels {
                    output: [1.0, 0.0],
                    ..Default::default()
                };
                mask.effects
                    .push(Effect::new(ids.next(), EffectKind::Levels(levels)));
            }
            layer.mask = Some(mask);
        }
        top = layer.id;
        stack.layers.push(layer);
    }
    let layer = stack.find_mut(top).expect("just added");
    for channel in [Channel::BaseColor, Channel::Roughness, Channel::Metallic] {
        let blend = layer.blending.get_mut(channel);
        if let Some(name) = option("blend") {
            blend.mode = *BlendMode::ALL
                .iter()
                .find(|m| {
                    m.label()
                        .split(" (")
                        .next()
                        .unwrap_or_default()
                        .replace(' ', "")
                        .eq_ignore_ascii_case(name)
                })
                .with_context(|| format!("unknown blend mode {name}"))?;
        }
        if let Some(opacity) = option("opacity") {
            blend.opacity = opacity.parse()?;
        }
    }
    if flag("folder") {
        let folder = stack.group(&[top], ids).expect("the layer is in the stack");
        if flag("passthrough") {
            let folder = stack.find_mut(folder).expect("just made");
            for channel in Channel::ALL {
                folder.blending.get_mut(channel).mode = BlendMode::Passthrough;
            }
        }
    }
    Ok(target)
}
