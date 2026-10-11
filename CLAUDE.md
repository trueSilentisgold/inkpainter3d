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
- Open question for the export phase: which Unity render pipeline (Built-in, URP, HDRP).

## Source of truth

- `docs/FEATURES.md` is the feature catalog: every feature with priority (`P0` core, `P1`
  everyday parity, `P2` later, `＋` beyond Painter), the reference project (a real Painter
  project the app must be able to rebuild), the architecture decisions and the phase plan.
  **Tick its checkboxes and update the phase table when a feature lands.**
- The README roadmap mirrors the phase table in English; keep both in sync.
- Phases 1 (viewport) and 2 (texture sets, HDRI lighting, Painter navigation) are done.
  Phase 3, the brush engine, is next.

## Parity rules

- Same concepts and terms as Painter (texture set, fill layer, smart mask, anchor point …),
  same default shortcuts (full table in `docs/FEATURES.md`, chapter 14). Navigation:
  Alt+LMB rotate (Shift snaps), Alt+MMB / Ctrl+Alt+LMB pan, Alt+RMB / wheel zoom,
  Shift+RMB rotates the environment, Ctrl+Alt+RMB picks the texture set, F frames,
  F5/F6 perspective/orthographic, M / C / Shift+C display modes, Alt+Q focus mode.
  Plain LMB is reserved for painting.
- Do not copy Adobe's name, logo, icons, shipped materials or brushes. `.sbsar` cannot be
  supported (proprietary engine); generators and filters are our own WGSL shaders.
- Content we ship must be self-made or CC0 (Poly Haven, ambientCG) and credited in the README.

## Architecture

| Crate | Contents |
|---|---|
| `ink_core` | `Mesh` (positions, normals, UVs, MikkTSpace tangents, indices) and `Model`: objects, texture sets and parts; `ModelBuilder` for importers; ray picking |
| `ink_io` | Model import (ufbx, gltf, tobj) into a `Model`; HDR environments (`.hdr`, `.exr`) including the bundled default |
| `ink_render` | `ViewportRenderer`, image based lighting (`environment.rs`), `OrbitCamera`, WGSL shaders |
| `ink_app` | eframe/egui application: texture set list, settings panels, viewport input |

Things that are easy to get wrong:

- **Model layout.** Every material slot becomes a `TextureSet` (sorted by name); every
  file object a `MeshObject`. `Model::parts` are contiguous index ranges, one per
  (object, texture set), grouped by texture set, then object. Visibility lives on both sets
  and objects (`Model::is_part_visible`).
- **UV convention.** UVs are stored with the origin top-left (like glTF/wgpu); importers
  flip FBX and OBJ. Tangents are computed in the bottom-left convention so the bitangent
  points up in the image, as OpenGL-style normal maps (Blender, Unity, Godot) expect.
- **Channel textures.** Per texture set the viewport samples four textures: base color +
  opacity (sRGB), tangent-space normal (OpenGL), roughness/metallic/AO, emissive (sRGB).
  Until the layer stack exists they are filled with `TextureSet::fill`.
- **egui color space.** egui expects gamma-encoded texture values. The viewport renders to an
  sRGB target and hands egui a non-sRGB (`Rgba8Unorm`) view of it; sampling the sRGB view
  makes the viewport look too dark.
- **IBL.** `IblBaker` uploads the panorama with a CPU mip chain, prefilters a GGX specular
  cube map on the GPU, projects irradiance onto 9 SH coefficients on the CPU, and a
  split-sum BRDF LUT is computed once. Shaders and Rust must agree on `equirect_uv` and the
  SH basis order.
- **Highlighting** uses the draw's first instance (1 = highlighted) instead of extra
  buffers.
- **WGSL** is validated by naga in `ink_render`'s tests. `set` is a reserved word.

Architecture decisions already taken (details in `docs/FEATURES.md`): brush strokes are
stored as data and rasterized (resolution can change any time, re-import keeps painting);
the layer stack composites on the GPU in 16-bit float; projects are a ZIP of JSON plus binary
data; undo is a command log; generators and filters are WGSL plus a parameter description.

## Checks

Run what CI runs (`.github/workflows/ci.yml`, Linux and Windows, on pushes to `main` and on
pull requests):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

## Seeing the app without a display

The container has no GPU or screen. Software Vulkan works:

```sh
apt-get install -y mesa-vulkan-drivers libxkbcommon-x11-0 xdotool   # lavapipe + X11 runtime
cargo run --release -p ink_render --example snapshot -- model.fbx out.png display=materialid
```

The snapshot example also takes `sphere` / `cube`, `yaw=`, `rough=`, `metal=`, `color=r,g,b`,
`env=file.hdr` and `toon`.

To drive the real UI, start `Xvfb :99`, run the app with `DISPLAY=:99`, and screenshot with
`import -window root`. With xdotool: call `xdotool windowfocus <id>` first or modifier keys
never arrive, and sleep between pointer moves (llvmpipe frames are slow, egui misses drags
that start and end in one frame). Don't `pkill -f` a pattern that also matches your own
shell command; use `pgrep -x inkpainter3d | xargs -r kill`.
