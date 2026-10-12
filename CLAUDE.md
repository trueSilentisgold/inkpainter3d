# inkpainter3d — notes for Claude

A free, open-source 3D texture painter meant to replace Adobe Substance 3D Painter in daily
work, on Linux first and Windows equally. Rust, wgpu, egui. GPL-3.0-or-later.

## Working with the owner

- **Talk to the owner in German.** Code, comments, commit messages and the README stay in
  English; `docs/FEATURES.md` is German.
- The owner is a texture artist who uses Painter (German UI) every day and textures
  everything, mostly **realistic PBR**; stylized / toon comes second. Target engines are
  **Blender, Unity and Godot**.
- The bar is **1:1 Painter parity** — if a workflow feels different from Painter, the owner
  won't switch. When in doubt, check how Painter does it (Adobe's docs on
  experienceleague.adobe.com) and match it.
- Never commit the owner's models, textures, screenshots or renders of their assets. Keep
  them in the session scratchpad.
- The export has templates for all three Unity render pipelines (Built-in, URP, HDRP), so
  the owner's choice doesn't matter for now.

## Source of truth

- `docs/FEATURES.md` is the feature catalog: every feature with a priority (`P0` core, `P1`
  everyday parity, `P2` later; `＋` marks features beyond Painter), the reference project (a
  real Painter project the app must be able to rebuild), the architecture decisions and the
  phase plan. The first row of its phase table ("Reihenfolge") without ✅ is the current
  phase.
- When a feature lands: tick its checkbox, delete notes there that describe the old state,
  update the phase table, and in the README the status line, "Available now", the roadmap
  table and, for new shortcuts, "Controls".
- Unfinished phase work may live on a `claude/*` branch with an open pull request; check
  before branching from `main`.

## Parity rules

- Same concepts and terms as Painter (texture set, fill layer, smart mask, anchor point …)
  and the same default shortcuts: all of Painter's are in `docs/FEATURES.md` chapter 14,
  the implemented ones in the README's "Controls". Plain LMB is reserved for painting.
- Do not copy Adobe's name, logo, icons, shipped materials or brushes. `.sbsar` cannot be
  supported (proprietary engine); generators and filters are our own WGSL shaders.
- Content we ship must be self-made or CC0 (Poly Haven, ambientCG) and credited in the README.

## Architecture

Layering: `ink_core` is the data model (no GPU, no file I/O); `ink_io` does everything that
reads or writes files; `ink_render` does everything wgpu and depends only on `ink_core`
(`ink_io` is a dev-dependency for the snapshot example); `ink_app` is the eframe/egui UI
(`app.rs` with `app/views.rs` for the 3D and 2D views, `app/painting.rs` for the brush,
`app/layers.rs` for the layer stack panel and the properties of fills and effects,
`app/project.rs` for the File menu's project commands and `app/export.rs` for the export
window)
plus `pen.rs`, pen input on Linux: a Wayland tablet client and an XInput2 pressure reader
(Linux-only dependencies).

The dependencies are newer than most training data: eframe/egui 0.36 (`App::ui(&mut self,
ui, frame)`, `egui::Panel::top/right`), wgpu and naga 30, glam 0.34. Look signatures up in
`~/.cargo/registry/src/*/<crate>-<version>` instead of writing them from memory. Use wgpu only
as `ink_render::wgpu` (re-exported from egui-wgpu); never add a direct `wgpu` dependency.

Things that are easy to get wrong:

- **Model layout.** Every material (by name) that has triangles becomes a `TextureSet`,
  sorted case-insensitively by name; triangles without a material go to `DefaultMaterial`.
  Every FBX/glTF mesh node becomes a `MeshObject` (names may repeat); OBJ objects are merged
  by name. Empty sets and objects are dropped. `Model::parts` are contiguous index ranges,
  one per (object, texture set), grouped by texture set, then object. Visibility lives on
  both sets and objects (`Model::is_part_visible`). Geometry is baked to world space, and
  the builder regroups triangles and splits vertices for tangents, so vertex and triangle
  indices do not match the source file.
- **No stable IDs yet.** Sets, objects and parts are identified by `Vec` index (UI state
  `expanded`/`focus`, the layer selection per set, the renderer's GPU sets, material ID
  colors, `Stroke::hidden_objects` / `hidden_sets`, the undo history, project files).
  Reordering or removing them must update all of these; a mesh re-import must remap them.
  Strokes, layers, masks, effects and paint layers do have ids (`Model::next_id`, unique
  per model).
- **UV convention.** UVs are stored with the origin top-left (like glTF/wgpu); importers
  flip FBX and OBJ. Tangents are computed in the bottom-left convention so the bitangent
  points up in the image, as OpenGL-style normal maps (Blender, Unity, Godot) expect.
- **Colors.** `ChannelValues` and `ViewportSettings` store sRGB floats; convert with
  `srgb_to_linear` before they reach uniforms or clear colors.
- **Channel textures** are `CHANNEL_FORMATS` in `viewport.rs` plus the `@group(1)` bindings
  in `viewport.wgsl`: color channels in sRGB textures, data channels linear, normals
  OpenGL-style; the normal texture's alpha carries the height. The compositor writes them;
  where no layer puts anything, Painter's shader defaults show (`background` in
  compositor.rs).
- **Layer stack** (`ink_core::layer`). Each `TextureSet` owns a `LayerStack`, stored bottom to
  top; folders hold their own lists. Layers, masks, effects and every `PaintLayer` (a paint
  layer's content, a mask's paint, a paint effect) have ids from `Model::ids`, unique per
  model; `Layer::duplicate` gives the copy and everything in it new ids. Blending is per
  channel (`Blending`); `BlendMode::for_layers` / `for_masks` say which modes each offers
  (Passthrough only for folders). Fill layers and fill effects hold `Arc<TextureImage>`s;
  the renderer uploads each image once by its id.
- **UI → GPU sync.** The UI mutates `Model` directly; `ViewportRenderer::sync_texture_sets`
  runs once per frame, after both views handled their input (`views()` in app/views.rs). It
  recreates a set's GPU state when the resolution changes, syncs its paint layers
  (`PaintCaches::sync_set`) and composites (`Compositor::composite`). New geometry goes
  through `set_model`; `render` draws no meshes while the GPU set count differs from the
  model's.
- **Strokes → GPU.** The app pushes a stroke onto the target `PaintLayer::strokes` when the
  button goes down and appends dabs while it moves. `PaintCaches` (paint.rs) diffs each paint
  layer's strokes against what its tiles hold by (stroke id, dab count): new dabs on the
  topmost stroke are redrawn over a snapshot of the tiles it touched (copied on first
  touch); undone strokes copy back what they painted over (the snapshot for the topmost,
  budgeted "pre-images" for earlier ones); anything else clears the layer and replays every
  stroke. Paint layers that leave the stack keep their tiles for a while (`ORPHANS_KEPT`),
  for undo. Changed texels are tracked as a `Region` of a few rectangles, not one bounding
  box. So only ever append dabs to the topmost stroke, give new strokes new ids, and keep
  everything that shapes the pixels inside the `Stroke` (brush values, camera, hidden
  objects): replays must give the same result.
- **Paint tiles** (tiles.rs). Painted pixels live in sparse 128² tiles in shared pools of
  2048² array pages (`POOL_FORMATS`: Rgba16Float for colors and normals, Rg16Float for
  scalars, masks and paint effects), premultiplied: value·coverage plus coverage. Each paint
  layer has a page table per channel in a storage buffer (`EMPTY` = no paint).
- **Compositor** (compositor.rs). Composites in 512² chunks plus a 1-texel apron: first the
  masks of the stack into a mask cache, then each channel's layers onto a 16-bit float
  accumulator (`composite.wgsl`); isolated folders composite onto an accumulator of their
  own, passthrough folders blend in place; a resolve pass writes the channel textures and
  derives the normal from the height; the mips follow. Every texel is evaluated at its
  "seed" (nearest texel inside a UV island, from a jump flood), which pads past seams. What
  triggers what: a change of `signature(stack)` (everything but strokes) queues a full
  composite that runs `CHUNKS_PER_FRAME` chunks per frame (`is_compositing()`; the app
  repaints while it is true); painted regions composite right away with an 8-texel margin,
  and the stroke's padding catches up to 64 texels when it ends. While a stroke is in
  progress, full composites wait. A new pixel-affecting field of a layer must go into
  `signature`.
- **Undo of layer edits.** `Command::EditStack` swaps the whole stack; strokes are shared
  `Arc`s, so the copy is cheap. The app clones the selected set's stack before the panels
  run (`begin_stack_edits`) and records a step when it changed (`end_stack_edits`), once no
  button is held and no text field is focused; strokes record their own `AddStroke`. Keep
  stack edits between those two calls, or commit the pending edit first
  (`commit_stack_edit`), as undo, redo and new strokes do.
- **Project files** (`ink_io::project`, described in `docs/project-format.md`). The file
  format is its own set of `File*` structs, mapped field by field to the model, with
  stable string keys (`Channel::key`, `BlendMode::key`). A new field of the model that
  shapes the pixels or is worth keeping must go into both directions of that mapping, the
  round-trip test and the format description; raise `VERSION` only when older readers
  could no longer read the files. Images are known by their ids; `ImageCache` keeps their
  encodings between saves. The app saves and opens on a thread with a clone of the model.
  Unsaved changes are `History::state` plus a hash of the settings that are no undo steps
  (`project_state` in `app/project.rs`): a saved setting that is no undo step belongs in
  that hash.
- **Export** (`ink_core::export`, `ink_io::export`). `ViewportRenderer::export_texture_set`
  composites a texture set chunk by chunk like the viewport and resolves it with
  `fs_export` (composite_resolve.wgsl) into four 16-bit float targets instead of the
  channel textures: linear colors, the normal with the height's detail (OpenGL), and each
  texel's distance to the nearest UV island for the padding. `MapImage::pack` turns those
  texels into an output map's pixels (packing, sRGB for colors in 8 and 16 bits, padding,
  normal format); `TEMPLATES` are the output templates. The app packs and writes on a
  thread per texture set while the GPU composites the next.
- **Buffers reused across draws** (stroke params, dabs, apply params) are written with
  `queue.write_buffer` and submitted per draw: writes land before the next `submit`, so
  several draws in one encoder would all see the last write.
- **Occlusion.** A surface stroke stores its `StrokeCamera`; the renderer draws a depth map
  for it at a fixed size (`DEPTH_MAP_SIZE`, not the window size) and skips texels behind it.
  Backface culling compares with the stamp normal, as in Painter.
- **egui color space.** egui expects gamma-encoded texture values. The viewport renders to an
  sRGB target and hands egui a non-sRGB (`Rgba8Unorm`) view of it; sampling the sRGB view
  makes the viewport look too dark.
- **Rust ↔ WGSL contracts** break the render without a compile error:
  `Uniforms`/`TextureSetUniforms` (viewport), `GpuDab`/`StrokeParams`/`DepthCamera`/
  `ApplyParams`/`TileDraw` (paint), `PassUniforms`/`ResolveUniforms`/`GeometryUniforms`
  (compositor) and `UvUniforms` (2D view) mirror the WGSL structs field for field (vec4/mat4
  only; update the size tests); `export_texel` in compositor.rs reads `ExportOutputs` of
  composite_resolve.wgsl, whose `APRON` is the compositor's; `BlendMode::code` is the `BLEND_*` constants in
  composite.wgsl (checked by `blend_codes_match_the_shader`); the order of
  `DisplayMode::ALL` is the `DISPLAY_*` code in viewport.wgsl and uv_view.wgsl;
  `UvCamera::to_pixel` and `to_clip` in uv_view.wgsl must agree;
  the alpha mode rides in `id_color.w`; highlighting uses the draw's first instance (1 =
  highlighted); the panorama mapping (`equirect_uv` in `viewport.wgsl` and
  `ibl_common.wgsl`, its inverse `equirect_direction` in `environment.rs`) and the SH basis
  order (`sh_basis` in Rust, `irradiance` in WGSL) must agree.
- **IBL.** `IblBaker` uploads the panorama with a CPU mip chain, prefilters a GGX specular
  cube map on the GPU and projects irradiance onto 9 SH coefficients on the CPU; a split-sum
  BRDF LUT is computed once.
- **WGSL validation.** The naga test `shaders_are_valid_wgsl` in `viewport.rs` checks only the
  sources it lists; add new shader files there. `prefilter.wgsl` and `brdf_lut.wgsl` compile
  only with `ibl_common.wgsl` prepended (`concat!`); the other shaders stand alone, hence
  their own copies of helpers like `equirect_uv` or `srgb_to_linear`. `set` and `target`
  are reserved words.
- **Pointer input.** Both views run `paint_input` over the same frame's events; a press only
  starts a stroke in the view whose response is hovered, Alt presses belong to navigation,
  and the views don't navigate while a stroke or Ctrl-drag runs. `end_lost_painting` ends a
  stroke whose view was hidden (F2 / F3), whose release got lost or when focus is lost.
  The start of Shift+click lines is kept on the model (world position or UV), not on screen.
  The views are not focusable and Tab is dropped, so keyboard focus can't swallow shortcuts.
  Ctrl+C / X / V never arrive as key presses: egui-winit turns them into `Event::Copy`,
  `Event::Cut` and `Event::Paste`, the last only while the system clipboard holds text, so
  copying layers also puts their names there (`sync_system_clipboard`).
  Pen pressure arrives as the force of `egui::Event::Touch`: from winit on Windows (Windows
  Ink), injected by `pen.rs` in `raw_input_hook` on Linux. On Wayland the pen reaches the app
  only through the tablet protocol, so `pen.rs` also injects its pointer events; on X11 the
  pen moves the pointer itself and `pen.rs` only adds the pressure.
- **Unit tests never touch a GPU.** wgpu validation errors (layouts, formats, pipeline
  state) only show at runtime: after any renderer change, run `scripts/gpu-smoke.sh`
  (below), which CI runs too, and render what the change affects with the snapshot example.

The architecture decisions for what comes next are settled; read `docs/FEATURES.md` →
"Architektur-Entscheidungen" before building the brush engine, layer stack, project files or
export. In short: strokes are stored as data and rasterized with a pixel cache; the layer
stack composites on the GPU in 16-bit float; the normal map format (OpenGL / DirectX) is a
per-project setting converted on export; projects are a ZIP of JSON plus binary data; undo
uses commands over the stroke data; generators and filters are WGSL plus a parameter
description.

## Checks

Run what CI runs (`.github/workflows/ci.yml`, Linux and Windows, on pushes to `main` and on
pull requests):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

Keep `--workspace`: `default-members` is `ink_app`, which has no tests, so a bare
`cargo test` or `cargo clippy` silently skips the library crates. CI uses the latest stable
Rust, whose Clippy may know lints your toolchain doesn't; `rustup toolchain install` it if
CI fails on one.

CI also runs `scripts/gpu-smoke.sh` on software Vulkan: it renders the snapshot example's
standard cases (painting, undo, occlusion, masks, blending, normal maps, projections,
project round trips, export padding, normal formats and packing) and checks a few pixels
of each. Add a case when you add something the renderer must keep
doing; its test images live in `crates/ink_render/examples/testdata/`.

## Seeing the app without a display

The container has no GPU or screen; software Vulkan (lavapipe) works. Xvfb and ImageMagick's
`import` are preinstalled.

```sh
apt-get install -y --no-install-recommends mesa-vulkan-drivers libvulkan1 \
  libxkbcommon-x11-0 libxcursor1 libxrandr2 libxi6 libx11-xcb1 xdotool
cargo run --release -p ink_render --example snapshot -- model.fbx out.png display=materialid
```

The snapshot options are listed at the top of `crates/ink_render/examples/snapshot.rs`; the
model may also be `sphere` or `cube`. `display=` takes a display mode label without spaces,
case-insensitive (`basecolor`, `ambientocclusion`, …). It also paints test strokes through
the real paint pipeline (`paint`, `erase`, `uv`, `live=`, `undo`, `nocull`, `nodepth`,
`set=`, `only=`), builds layer stacks to check the compositor (`fill=`, `image=`,
`normalmap=`, `heightmap=`, `triplanar`, `mask`, `levels`, `blend=`, `opacity=`, `folder`,
`passthrough`, `height`, `rotate=`, `nblend=`, `nopacity=`), renders the 2D view (`2d`),
saves and opens projects (`save=file.ink3d`; the model may be a `.ink3d`), exports a
template (`export=unity_hdrp_metallic`, `format=`, `depth=`, `padding=`, `dilation=`) and
prints pixels for scripted checks (`probe=x,y` of the render, `xprobe=Map,x,y` of an
exported map). A small OBJ with two parallel quads in
different materials is the quickest occlusion test.

To check that the app starts on Wayland, install `weston`, run
`weston --backend=headless --socket=wl-test` with `XDG_RUNTIME_DIR` set to a private
directory and start the app with `WAYLAND_DISPLAY=wl-test DISPLAY=`. Headless Weston has no
tablet, so pens can't be tested here.

To drive the real UI, start `Xvfb :99`, run the app with `DISPLAY=:99` and the model as its
first argument (the file dialogs do nothing here: no xdg portal or zenity), and screenshot
with `import -window root`. With xdotool: call `xdotool windowfocus <id>` first or modifier
keys never arrive, and sleep between pointer moves (llvmpipe frames are slow, egui misses
drags that start and end in one frame). Don't `pkill -f` a pattern that also matches your
own shell command; use `pgrep -x inkpainter3d | xargs -r kill`.
