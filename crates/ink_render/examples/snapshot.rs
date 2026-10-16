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
//! `heightmap=file.png` (its height image), `triplanar`, `scale=4` and `rotate=30` (its
//! projection);
//! `mask` (the fill gets a black mask and the strokes paint into it), `levels` (an inverting
//! levels effect in that mask), `blur=0.3` (a blur with this intensity on top of that mask,
//! blended with Replace); `blend=multiply` and `opacity=0.5` (blending of the top
//! layer's color, roughness and metallic), `nblend=` and `nopacity=` (of its normal);
//! `folder` (puts the top layer in a folder), `passthrough` (that folder passes through);
//! `height` (strokes paint only height 1).
//!
//! Brush strokes through the middle of the view, to check the paint pipeline: `paint`
//! (horizontal, red and shiny), `erase` (vertical eraser stroke), `uv` (a diagonal stroke
//! in UV space), `size=8`, `hardness=0.8`, `tip=round|square`, `res=1024` (texture size),
//! `live=10` (paints in chunks of 10 dabs like the app does), `undo` or `undo=2` (removes
//! the last strokes again), `nocull` (paints surfaces facing away too), `nodepth` (paints hidden
//! surfaces too), `set=Name` (texture set to paint; default: the one in the middle),
//! `only=Name` (shows only this texture set after painting). `2d` renders the 2D (UV) view
//! of the painted texture set instead.
//!
//! `align=wrap|planar|camera` (the brush's alignment), `follow` (Follow Path), `jitter=0.5`
//! (all four jitter amounts) and `angle=30` apply to those strokes; `mirror=x` (any of
//! `xyz`) or `radial=4` (around Y) paints the surface strokes with symmetry about the origin.
//!
//! `polyfill=triangle|polygon|mesh|island` fills what the middle of the view shows with a
//! Polygon Fill stroke (blue), after the other strokes.
//!
//! `stencil=half` paints the strokes through a stencil whose left half is white and whose
//! right half is black, centered in the view.
//!
//! Baking: `bake=ao,curvature,...` (keys of `MeshMap::key`, or `all`) bakes mesh maps of the
//! texture set (`set=Name`, else the first) and writes them as PNGs into `<out>.bake/`;
//! `bakehigh=file.obj` is the high-poly, `rays=16`, `sub=2`, `dilation=8`, `front=0.1` and `rear=0.1` tune it, and
//! `bprobe=map,x,y` prints a texel as `bprobe map x y r g b` (8 bits).
//!
//! Generators: `generator=edge_wear` (a `GeneratorKind::key`) puts a red fill layer with a black
//! mask holding that generator on top; `gparam=key:value` (may be given several times) sets
//! its parameters. Bake the mesh maps it reads in the same run with `bake=`.
//! `colorsel=243,85,85` puts a red fill layer on top whose black mask holds a color
//! selection of that ID map color (8 bits; `csinvert` inverts it); bake `id` with it.
//! `smart=Rusty_metal` puts a starter preset (`ink_io::starter`, by name with `_` for spaces)
//! on top of the stack.
//!
//! `probe=x,y` (may be given several times) prints the pixel there as `probe x y r g b`,
//! for scripted checks (scripts/gpu-smoke.sh).
//!
//! Projects: the model may be a project file (`.ink3d`); `save=file.ink3d` saves the model
//! with its strokes as one. Export: `export=pbr_metallic_roughness` (a template key from
//! `ink_core::export::TEMPLATES`) writes the painted texture set's maps into the directory
//! `<out>.export/`, with `format=png|targa|jpeg|tiff|exr`, `depth=8|8_dithered|16|16f|32f`,
//! `padding=passthrough|infinite|transparent|color` and `dilation=16`; `xprobe=Name,x,y`
//! prints the texel of the exported map whose name ends with `Name` as
//! `probe x y r g b a` (8 bits; after the `probe=` lines).

use anyhow::{Context, Result};
use glam::Vec2;
use ink_core::export::{BitDepth, ExportSettings, FileFormat, MapImage, Padding, Template};
use ink_core::{
    Alignment, Alpha, BlendMode, Blur, Brush, BrushMaterial, Channel, ChannelSet, Content, Effect,
    EffectKind, Layer, LayerKind, Levels, Mask, Mesh, Model, ProjectionMode, Stroke, StrokePath,
    StrokeSpace, Tool, shape_dabs, surface_dab, uv_dab,
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
    // Like the app: everything the GPU can do.
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: adapter.limits(),
        ..Default::default()
    }))?;

    let mut renderer = ViewportRenderer::new(&device, &queue);
    let env = match option("env") {
        Some(path) => ink_io::load_environment(Path::new(path))?,
        None => ink_io::default_environment(),
    };
    renderer.set_environment(&device, &queue, env.width, env.height, &env.pixels);
    let mut model = match model_path.as_str() {
        "sphere" => Model::from_mesh(Mesh::uv_sphere(1.0, 96, 48)),
        "cube" => Model::from_mesh(Mesh::cube(1.5)),
        path if path.ends_with(".ink3d") => ink_io::project::load(Path::new(path))?.0.model,
        path => ink_io::load_model(Path::new(path))?,
    };
    if let Some(res) = option("res") {
        let res: u32 = res.parse()?;
        for set in &mut model.texture_sets {
            set.resolution = res;
        }
    }
    if let Some(spec) = option("bake") {
        bake_maps(&device, &queue, &mut model, &args[3..], spec, out_path)?;
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
        angle: float("angle")?.unwrap_or(0.0),
        follow_path: flag("follow"),
        jitter: {
            let j = float("jitter")?.unwrap_or(0.0);
            ink_core::Jitter {
                size: j,
                flow: j,
                angle: j,
                position: j,
            }
        },
        alignment: match option("align") {
            Some("planar") => Alignment::TangentPlanar,
            Some("camera") => Alignment::Camera,
            _ => Alignment::TangentWrap,
        },
        alpha: match option("tip") {
            Some("square") => Alpha::Square,
            _ => Alpha::Round,
        },
        ..Default::default()
    };
    let live: usize = option("live").map_or(Ok(usize::MAX), str::parse)?;
    let mut symmetry = ink_core::Symmetry::default();
    if let Some(axes) = option("mirror") {
        symmetry.enabled = true;
        symmetry.mirror = ['x', 'y', 'z'].map(|a| axes.contains(a));
    }
    if let Some(count) = option("radial") {
        symmetry.enabled = true;
        symmetry.mode = ink_core::SymmetryMode::Radial;
        symmetry.count = count.parse()?;
    }
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
    if let Some(key) = option("generator") {
        let kind = ink_core::GeneratorKind::from_key(key)
            .with_context(|| format!("unknown generator {key}"))?;
        let mut generator = ink_core::Generator::new(kind);
        for a in &args[3..] {
            if let Some((k, v)) = a.strip_prefix("gparam=").and_then(|p| p.split_once(':')) {
                generator.set(k, v.parse()?);
            }
        }
        let mut layer = Layer::new(LayerKind::Fill, "Generator".into(), &mut model.ids);
        if let Content::Fill(fill) = &mut layer.content {
            fill.values.base_color = [0.9, 0.1, 0.1];
        }
        let mut mask = Mask::new(0.0, model.next_id());
        mask.effects.push(Effect::new(
            model.next_id(),
            EffectKind::Generator(generator),
        ));
        layer.mask = Some(mask);
        model.texture_sets[set].stack.layers.push(layer);
    }
    if let Some(rgb) = option("colorsel") {
        let mut selection = ink_core::ColorSelection::default();
        let parts: Vec<u8> = rgb.split(',').map(str::parse).collect::<Result<_, _>>()?;
        anyhow::ensure!(parts.len() == 3, "colorsel takes r,g,b in 8 bits");
        selection.colors.push([parts[0], parts[1], parts[2]]);
        selection.invert = flag("csinvert");
        let mut layer = Layer::new(LayerKind::Fill, "Selected".into(), &mut model.ids);
        if let Content::Fill(fill) = &mut layer.content {
            fill.values.base_color = [0.9, 0.1, 0.1];
        }
        let mut mask = Mask::new(0.0, model.next_id());
        mask.effects.push(Effect::new(
            model.next_id(),
            EffectKind::ColorSelection(selection),
        ));
        layer.mask = Some(mask);
        model.texture_sets[set].stack.layers.push(layer);
    }
    if let Some(name) = option("smart") {
        let preset = ink_io::starter::all()
            .into_iter()
            .find(|p| p.name.eq_ignore_ascii_case(&name.replace('_', " ")))
            .with_context(|| format!("no starter preset {name}"))?;
        for layer in &preset.layers {
            let copy = layer.duplicate(&mut model.ids);
            model.texture_sets[set].stack.layers.push(copy);
        }
    }
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
                    brush.alignment,
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
        let stroke_id = model.next_id();
        let stencil =
            (option("stencil") == Some("half") && space == StrokeSpace::Surface).then(|| {
                let image = Arc::new(ink_core::AlphaImage::new(
                    "half".into(),
                    64,
                    32,
                    (0..64 * 32)
                        .map(|i| if i % 64 < 32 { 255 } else { 0 })
                        .collect(),
                ));
                let stencil = ink_core::Stencil {
                    image: Some(image.clone()),
                    size: 1.2,
                    ..Default::default()
                };
                ink_core::StrokeStencil {
                    image,
                    affine: stencil.view_affine().unwrap(),
                }
            });
        shape_dabs(&brush, space, stroke_id, 0, None, &mut dabs);
        let stroke = Stroke {
            id: stroke_id,
            tool,
            space,
            alpha: brush.alpha.clone(),
            hardness: brush.hardness,
            alignment: brush.alignment,
            opacity: brush.opacity,
            backface_cos: brush.backface_cos(),
            material,
            camera: (space == StrokeSpace::Surface && !flag("nodepth"))
                .then(|| camera.stroke_camera(view.x / view.y)),
            hidden_objects: Vec::new(),
            hidden_sets: Vec::new(),
            fill: None,
            stencil,
            symmetry: if space == StrokeSpace::Surface {
                symmetry.copies()
            } else {
                Vec::new()
            },
            dabs: Vec::new(),
        };
        let paint = target;
        let layer = model.texture_sets[set].stack.paint_mut(paint).unwrap();
        layer.strokes.push(std::sync::Arc::new(stroke));
        for chunk in dabs.chunks(live) {
            let layer = model.texture_sets[set].stack.paint_mut(paint).unwrap();
            layer.extend_stroke(stroke_id, chunk.iter().copied());
            renderer.sync_texture_sets(&device, &queue, &model, true);
        }
    }
    if let (Some(shape), Some(hit)) = (option("polyfill"), &center) {
        let shape = match shape {
            "triangle" => ink_core::FillShape::Triangle,
            "mesh" => ink_core::FillShape::Mesh,
            "island" => ink_core::FillShape::UvIsland,
            _ => ink_core::FillShape::Polygon,
        };
        let mut material = BrushMaterial::default();
        material.values.base_color = [0.1, 0.3, 1.0];
        let stroke = Stroke {
            id: model.next_id(),
            tool: Tool::Paint,
            space: StrokeSpace::Surface,
            alpha: Alpha::Round,
            hardness: 1.0,
            alignment: Alignment::TangentWrap,
            opacity: 1.0,
            backface_cos: -1.0,
            material,
            camera: None,
            hidden_objects: Vec::new(),
            hidden_sets: Vec::new(),
            fill: Some(model.fill_target(shape, hit.triangle as u32)),
            stencil: None,
            symmetry: Vec::new(),
            dabs: Vec::new(),
        };
        let layer = model.texture_sets[set].stack.paint_mut(target).unwrap();
        layer.strokes.push(std::sync::Arc::new(stroke));
        renderer.sync_texture_sets(&device, &queue, &model, true);
    }
    if let Some(path) = option("save") {
        let project = ink_io::project::Project {
            model: model.clone(),
            source: None,
            view: None,
            export: None,
            bake: None,
        };
        ink_io::project::save(Path::new(path), &project, &mut Default::default())?;
        eprintln!("Saved {path}");
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
    let image = image::RgbaImage::from_raw(SIZE[0], SIZE[1], pixels).context("image size")?;
    image.save(out_path)?;
    eprintln!("Wrote {out_path}");
    for probe in args[3..].iter().filter_map(|a| a.strip_prefix("probe=")) {
        let (x, y) = probe
            .split_once(',')
            .with_context(|| format!("probe={probe} is not x,y"))?;
        let (x, y): (u32, u32) = (x.parse()?, y.parse()?);
        let [r, g, b, _] = image.get_pixel(x, y).0;
        println!("probe {x} {y} {r} {g} {b}");
    }

    if let Some(key) = option("export") {
        let template =
            Template::from_key(key).with_context(|| format!("unknown template {key}"))?;
        let format = match option("format") {
            Some(f) => FileFormat::from_key(f).with_context(|| format!("unknown format {f}"))?,
            None => FileFormat::Png,
        };
        let settings = ExportSettings {
            directory: String::new(),
            template,
            format,
            bit_depth: match option("depth") {
                Some(d) => BitDepth::from_key(d).with_context(|| format!("unknown depth {d}"))?,
                None => format.bit_depths()[0],
            },
            padding: match option("padding") {
                Some(p) => Padding::from_key(p).with_context(|| format!("unknown padding {p}"))?,
                None => Padding::Infinite,
            },
            dilation: option("dilation").map_or(Ok(16), str::parse)?,
        };
        let dir = Path::new(out_path).with_extension("export");
        std::fs::create_dir_all(&dir)?;
        let texture_set = &model.texture_sets[set];
        let mut maps: Vec<MapImage> = template
            .maps
            .iter()
            .map(|m| {
                MapImage::new(
                    texture_set.resolution,
                    settings.components(m),
                    settings.bit_depth,
                )
            })
            .collect();
        let started = std::time::Instant::now();
        renderer.sync_texture_sets(&device, &queue, &model, false);
        let blocks = renderer.export_blocks(set);
        renderer
            .export_texture_set(&device, &queue, &model, set, 0..blocks, &mut |block| {
                for (image, map) in maps.iter_mut().zip(template.maps) {
                    image.pack(&block, map, &settings, model.normal_format);
                }
            })
            .map_err(anyhow::Error::msg)?;
        eprintln!("Export took {:?}", started.elapsed());
        let mut written = Vec::new();
        for (image, map) in maps.into_iter().zip(template.maps) {
            let name = ink_core::export::file_name(map.name, &model.name, &texture_set.name, "");
            let path = dir.join(format!("{name}.{}", format.extension()));
            ink_io::export::write_map(&path, &image, format, settings.bit_depth)?;
            eprintln!("Exported {}", path.display());
            written.push((map.name, path));
        }
        for probe in args[3..].iter().filter_map(|a| a.strip_prefix("xprobe=")) {
            let mut parts = probe.split(',');
            let (Some(name), Some(x), Some(y)) = (parts.next(), parts.next(), parts.next()) else {
                anyhow::bail!("xprobe={probe} is not Name,x,y");
            };
            let (x, y): (u32, u32) = (x.parse()?, y.parse()?);
            let (_, path) = written
                .iter()
                .find(|(map, _)| map.ends_with(name))
                .with_context(|| format!("no exported map {name}"))?;
            let image = image::open(path)?.into_rgba8();
            let [r, g, b, a] = image.get_pixel(x, y).0;
            println!("probe {x} {y} {r} {g} {b} {a}");
        }
    }
    Ok(())
}

/// Builds the texture set's layer stack from the options. Returns the paint layer the
/// strokes go into.
fn bake_maps(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    model: &mut Model,
    args: &[String],
    spec: &str,
    out_path: &str,
) -> Result<()> {
    use ink_core::{BakeSettings, HighPoly, MeshMap, Texels};
    let option = |key: &str| {
        args.iter()
            .find_map(|a| a.strip_prefix(key)?.strip_prefix('='))
    };
    let mut settings = BakeSettings::default();
    if spec != "all" {
        settings.maps = spec.split(',').filter_map(MeshMap::from_key).collect();
    }
    if let Some(rays) = option("rays") {
        settings.ao_rays = rays.parse()?;
        settings.thickness_rays = settings.ao_rays;
    }
    if let Some(sub) = option("sub") {
        settings.subsampling = sub.parse()?;
    }
    if let Some(dilation) = option("dilation") {
        settings.dilation = dilation.parse()?;
    }
    if let Some(front) = option("front") {
        settings.front_distance = front.parse()?;
    }
    if let Some(rear) = option("rear") {
        settings.rear_distance = rear.parse()?;
    }
    let high = option("bakehigh")
        .map(|path| ink_io::load_model(Path::new(path)).map(|m| HighPoly::from_model(&m)))
        .transpose()?;
    let set = match option("set") {
        Some(name) => model
            .texture_sets
            .iter()
            .position(|s| s.name.starts_with(name))
            .with_context(|| format!("no texture set {name}"))?,
        None => 0,
    };
    let baker = ink_render::bake::Baker::new(device);
    let started = std::time::Instant::now();
    let maps = baker
        .bake(
            device,
            queue,
            &ink_render::bake::BakeJob {
                model,
                set,
                settings: &settings,
                high: high.as_ref(),
            },
            &mut |_| true,
        )
        .map_err(anyhow::Error::msg)?;
    eprintln!("Baked {} maps in {:.2?}", maps.len(), started.elapsed());
    let dir = format!("{out_path}.bake");
    std::fs::create_dir_all(&dir)?;
    let probes: Vec<(String, u32, u32)> = args
        .iter()
        .filter_map(|a| {
            let v = a.strip_prefix("bprobe=")?.to_owned();
            let mut parts = v.split(',');
            Some((
                parts.next()?.to_owned(),
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
            ))
        })
        .collect();
    for (map, image) in maps {
        let Texels::Rgba16(texels) = &image.texels else {
            continue;
        };
        let rgba8: Vec<u8> = texels.iter().map(|&v| (v >> 8) as u8).collect();
        image::save_buffer(
            format!("{dir}/{}.png", map.key()),
            &rgba8,
            image.width,
            image.height,
            image::ExtendedColorType::Rgba8,
        )?;
        for (name, x, y) in &probes {
            if name == map.key() {
                let i = ((y * image.width + x) * 4) as usize;
                println!(
                    "bprobe {name} {x} {y} {} {} {}",
                    rgba8[i],
                    rgba8[i + 1],
                    rgba8[i + 2]
                );
            }
        }
        model.texture_sets[set]
            .mesh_maps
            .insert(map, Arc::new(image));
    }
    Ok(())
}

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
    // New stacks hold a paint layer; projects may not.
    let mut target = stack
        .iter()
        .find_map(|l| match &l.content {
            Content::Paint(p) => Some(p.id),
            _ => None,
        })
        .unwrap_or(0);

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
            if let Some(angle) = floats("rotate")? {
                fill.projection.rotation = angle[0];
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
            if let Some(intensity) = floats("blur")? {
                // Replace, as in the reference project.
                let mut blur = Effect::new(
                    ids.next(),
                    EffectKind::Blur(Blur {
                        intensity: intensity[0],
                    }),
                );
                blur.blend = BlendMode::Replace;
                mask.effects.push(blur);
            }
            layer.mask = Some(mask);
        }
        top = layer.id;
        stack.layers.push(layer);
    }
    let layer = stack.find_mut(top).expect("just added");
    for (channel, prefix) in [
        (Channel::BaseColor, ""),
        (Channel::Roughness, ""),
        (Channel::Metallic, ""),
        (Channel::Normal, "n"),
    ] {
        let blend = layer.blending.get_mut(channel);
        let option = |key: &str| option(&format!("{prefix}{key}"));
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
