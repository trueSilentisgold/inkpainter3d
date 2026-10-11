# inkpainter3d

**A free, open-source 3D texture painting tool — in the spirit of Substance Painter, built for both realistic PBR and stylized / ink art.**

inkpainter3d lets you paint directly onto 3D models with a layer-based, non-destructive workflow
and export the result as texture maps for game engines and renderers. It runs natively on
**Windows and Linux** and will always be **free**.

> **Status:** early development. Milestone 1 (viewport and model import) is done; painting is next.

---

## Vision

Professional texture painting tools are expensive, closed and tied to a subscription.
inkpainter3d aims to be a capable alternative that:

- **is free and open** — no subscription, no account, no lock-in
- **runs everywhere artists work** — Windows and Linux as first-class platforms
- **treats stylized art as a first-class citizen**, not an afterthought: hand-painted looks,
  toon shading and clean ink lines are just as important as photorealistic PBR
- **feels like a painting tool** — responsive brushes, full graphics tablet support
  (pressure, tilt), fast GPU rendering
- **goes beyond pixels** with editable **vector paths drawn directly on the model surface**

### What makes it different

| | |
|---|---|
| **PBR and stylized in one** | Layers produce the same channels (base color, roughness, metallic, normal, ink …). Only the viewport shading changes, so you paint once and preview realistic or toon. |
| **Vector layers on the surface** | Bézier paths whose control points are anchored on the mesh itself (triangle + barycentric position). They stay editable, resolution independent and cross UV seams cleanly — ideal for ornaments, seams, panel lines and line art. |
| **Ink outline in the viewport** | Stylized mode renders a constant-width ink outline around the model, so you judge your textures the way they will look in a toon-shaded game. |

## Features

### Available now
- Model import: **FBX** (binary and ASCII), **glTF** (`.gltf`, `.glb`), **OBJ**
  - automatic axis and unit conversion (Z-up, centimeters, … → Y-up meters)
  - triangulation of n-gons, node hierarchy transforms, mirrored objects
- Viewport with two shading modes
  - **PBR** (metallic / roughness, GGX)
  - **Stylized**: banded toon shading, rim light, ink outline with adjustable color and width
- Orbit / pan / zoom camera, drag-and-drop loading

### Planned
See the [roadmap](#roadmap).

## Roadmap

| # | Milestone | Status |
|---|---|---|
| 1 | **Viewport** — model import (FBX, glTF, OBJ), PBR and stylized shading, camera | ✅ done |
| 2 | **Brush engine** — paint directly on the model via GPU projection, tablet pressure and tilt, brush spacing and smoothing | ⏳ next |
| 3 | **Layer stack** — paint and fill layers, opacity, blend modes, masks, groups, undo/redo | |
| 4 | **Channels and export** — roughness, metallic, normal, height; PNG/EXR export with presets for Unreal, Unity, Godot, Blender | |
| 5 | **Stylized tools** — ink channel, more toon controls, smoothed outline normals | |
| 6 | **Vector layers** — editable paths anchored on the surface, shapes, stroke styles, SVG decals | |
| 7 | **Baking and smart materials** — AO, curvature, ID and normal maps; generators such as edge wear | |
| 8 | **Releases** — installers for Windows, AppImage / Flatpak for Linux | |

## Building

Requires a recent stable [Rust toolchain](https://rustup.rs). On Windows you also need the
Visual Studio C++ Build Tools.

```sh
cargo run --release                     # opens with a default sphere
cargo run --release -- model.fbx        # opens a model directly
```

On Linux you need the usual windowing development packages, e.g. on Debian/Ubuntu:

```sh
sudo apt install libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgtk-3-dev
```

Models can also be opened via **File → Open model…** or dropped onto the window.

## Controls

| Action | Input |
|---|---|
| Orbit | Alt + left mouse, or right mouse |
| Pan | Middle mouse, or Shift + right mouse |
| Zoom | Mouse wheel |
| Frame model | F |

The left mouse button on its own is reserved for painting.

## Architecture

Written in Rust on top of [wgpu](https://wgpu.rs) (Vulkan / DirectX 12 / Metal) and
[egui](https://github.com/emilk/egui).

| Crate | Purpose |
|---|---|
| `ink_core` | Data model: meshes; later the layer stack, masks, vector paths and undo |
| `ink_io` | Model import (ufbx, gltf, tobj); later texture export |
| `ink_render` | wgpu viewport renderer, camera and WGSL shaders |
| `ink_app` | Application window and UI (eframe / egui) |

CI builds, lints and tests every push on Windows and Linux.

## License

[GPL-3.0-or-later](https://www.gnu.org/licenses/gpl-3.0.html) — inkpainter3d and any
derivative of it stay free and open.
