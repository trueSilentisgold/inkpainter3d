# inkpainter3d

**A free, open-source 3D texture painting tool — in the spirit of Substance Painter, built for both realistic PBR and stylized / ink art.**

inkpainter3d lets you paint directly onto 3D models with a layer-based, non-destructive workflow
and export the result as texture maps for game engines and renderers. It runs natively on
**Windows and Linux** and will always be **free**.

> **Status:** early development. Phases 1 to 5 (viewport, texture sets, HDRI lighting, brush engine, layer stack, project files and export) are done: a game asset can be textured and exported end to end. Baking is next.

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
  - every material becomes a **texture set**, object names are kept
  - automatic axis and unit conversion (Z-up, centimeters, … → Y-up meters)
  - triangulation of n-gons, node hierarchy transforms, mirrored objects
  - MikkTSpace tangents, so normal maps match Blender, Unity and Godot
- **Texture set list** like Painter's, plus more
  - show / hide per texture set, show all, hide all, invert, focus mode (Alt+Q)
  - expand a texture set to see which meshes use it, and show / hide single meshes
  - hovering a texture set or mesh highlights it in the viewport
  - select the texture set under the cursor with Ctrl+Alt+right click
- Texture set settings: size (128 to 4096), opacity mode (opaque, alpha test, alpha blend)
- Viewport
  - **PBR** with image based lighting from HDR panoramas (`.hdr`, `.exr`), rotation,
    exposure, background opacity and blur; a CC0 studio HDRI is built in
  - **Stylized**: banded toon shading, rim light, ink outline with adjustable color and width
  - display modes: material, material ID (one color per texture set) and single channels
  - perspective and orthographic camera, drag-and-drop loading of models and HDRIs
- **Brush engine** with Painter's tools and defaults
  - **Paint** (1) and **Eraser** (2) into the selected paint layer, mask or paint effect, in
    the 3D view and in the **2D view** (UV layout; layouts F1 to F4)
  - brush material per channel (base color, roughness, metallic, emissive, opacity), each
    channel switchable
  - size (relative to the model or the view), flow, stroke opacity, spacing, angle,
    hardness, backface culling; round and square tips or any grayscale image as alpha
  - **pen pressure** on size and flow with minimums and a pressure curve: Windows Ink on
    Windows, the tablet protocol on Wayland (GNOME, KDE), XInput2 on X11; on Wayland the
    pen's eraser end erases
  - **lazy mouse**, straight lines (Shift + click, snapped with Ctrl + Shift), undo and redo
    per stroke
  - strokes are stored as data and painted on the GPU: changing the texture size repaints
    them sharp; the tip wraps around the surface, and surfaces hidden from the camera stay
    unpainted
- **Layer stack** like Painter's, composited on the GPU in 16-bit float
  - **paint layers**, **fill layers** and **folders**; the topmost layer wins where its mask
    lets it, per channel
  - **blend mode and opacity per channel**, with a dropdown choosing the channel the rows
    show, like Painter's; all of Painter's blend modes, including Passthrough for folders
    and the normal map modes
  - **masks** (white or black) on layers and folders, painted with a grayscale brush, with
    paint, fill and levels effects, reordered by drag and drop and copied like layers;
    Shift + click turns a mask off
  - **fill layers** with a value or an image per channel (PNG, JPEG, TGA, BMP, EXR, HDR;
    normal maps in DirectX or OpenGL format), **UV** or **tri-planar** projection with
    scale, rotation, offset and hardness; height becomes normal detail
  - add, delete, duplicate, group, copy, cut, paste and rename layers, reorder them by drag
    and drop, show and hide them; every edit can be undone
- **Projects** (`.ink3d`) like Painter's: new project from a mesh with resolution and normal
  map format, open, recent files, close, save, save as, save as copy
  - one file with the mesh, layers, strokes, images and brush alphas; it opens without the
    original files, and strokes stay editable data
  - asks before unsaved changes are lost; the title shows a star for them
  - autosave: a backup every 10 minutes while there are unsaved changes, next to the
    project (the last 3 are kept)
  - the format is open and documented: [docs/project-format.md](docs/project-format.md)
- **Export textures** (Ctrl+Shift+E) like Painter's export window
  - output templates for **Blender** (Principled BSDF), **Unity** (HDRP, URP and
    Built-in), **Godot** (ORM), **glTF** PBR Metal Roughness, **Unreal Engine** (packed),
    PBR Metallic Roughness and the document channels
  - PNG (8 / 16 bit), TGA, JPEG, TIFF (8 / 16 bit), OpenEXR (16 / 32 bit float); 8 bit with
    dithering against banding
  - padding: infinite dilation, dilation by a distance with a transparent or the default
    background, or none
  - choose the texture sets, see every file before it is written, follow the export in the
    list of exports

### Planned
See the [roadmap](#roadmap).

## Roadmap

The goal is feature parity with Substance 3D Painter for everyday work: same concepts, same
terms, same shortcuts. The full feature catalog with priorities lives in
[docs/FEATURES.md](docs/FEATURES.md) (German).

| # | Phase | Status |
|---|---|---|
| 1 | **Viewport** — model import (FBX, glTF, OBJ), PBR and stylized shading, camera | ✅ done |
| 2 | **Foundation** — texture sets and meshes from import, texture set list with visibility per set and per mesh, material ID view, tangents, channel textures (incl. emissive and opacity), HDRI lighting, Painter-style navigation | ✅ done |
| 3 | **Brush engine** — paint and erase, strokes stored as data, pen pressure, alphas, lazy mouse, straight lines, undo, 2D view | ✅ done |
| 4 | **Layer stack** — paint, fill and group layers, per-channel blend modes and opacity, masks, UV and tri-planar projection, channel view | ✅ done |
| 5 | **Project and export** — project file, export templates for Blender, Unity, Godot, glTF | ✅ done |
| 6 | **Baking** — AO, curvature, position, thickness, world normal; normal, height and ID from high poly | ⏳ next |
| 7 | **Generators and library** — mask builder, edge wear, dirt, filters, noises, smart materials and masks, shelf, starter content | |
| 8 | **Remaining tools** — projection, polygon fill, symmetry, stencil, color selection, geometry mask, anchor points | |
| 9 | **Stylized and vector layers** — ink channel, toon controls, editable paths on the surface | |
| 10 | **Polish** — UDIM, smudge and clone, path tools, post effects, Python API, installers (AppImage / Flatpak, Windows) | |

## Building

Requires a recent stable [Rust toolchain](https://rustup.rs). On Windows you also need the
Visual Studio C++ Build Tools.

```sh
cargo run --release                     # opens with a default sphere
cargo run --release -- model.fbx        # starts a project from a model
cargo run --release -- project.ink3d    # opens a project
```

On Linux you need the usual windowing development packages, e.g. on Debian/Ubuntu:

```sh
sudo apt install libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgtk-3-dev
```

Projects start from a model with **File → New…**, or by dropping the model onto the window;
projects open via **File → Open…** or the same way. HDR panoramas load via
**File → Load environment…** or by dropping them, too.

To check rendering without a window, e.g. on a server, render a snapshot to PNG:

```sh
cargo run --release -p ink_render --example snapshot -- model.fbx out.png display=materialid
cargo run --release -p ink_render --example snapshot -- sphere out.png paint erase   # test strokes
cargo run --release -p ink_render --example snapshot -- project.ink3d out.png export=unity_hdrp_metallic
```

## Controls

The same as in Substance 3D Painter.

| Action | Input |
|---|---|
| New / open / save / close project | Ctrl + N / Ctrl + O / Ctrl + S / Ctrl + F4 |
| Export textures | Ctrl + Shift + E |
| Paint / Eraser | 1 / 2, then left mouse |
| Straight line from the last stroke | Shift + click (Ctrl + Shift snaps to 15°) |
| Brush size | [ and ], or Ctrl + right mouse drag sideways |
| Hardness | Ctrl + right mouse drag up and down |
| Flow / angle | Ctrl + left mouse drag sideways / up and down |
| Lazy mouse on / off | D |
| Paint black instead of white in a mask (and back) | X |
| Undo / redo | Ctrl + Z / Ctrl + Y |
| Layouts: 3D and 2D / 3D / 2D / swap | F1 / F2 / F3 / F4 |
| Rotate | Alt + left mouse (add Shift to snap to 90°) |
| Pan | Alt + middle mouse, or Ctrl + Alt + left mouse |
| Zoom | Alt + right mouse, or mouse wheel |
| Rotate environment | Shift + right mouse |
| Frame model and UV square | F |
| Perspective / orthographic | F5 / F6 |
| Material / next channel / previous channel | M / C / Shift + C |
| Focus on selected texture set | Alt + Q |
| Select texture set under cursor | Ctrl + Alt + right click |
| Duplicate / group layers | Ctrl + D / Ctrl + G |
| Copy / cut / paste / delete layers (pointer over the layer stack) | Ctrl + C / Ctrl + X / Ctrl + V / Delete |
| Select several layers | Ctrl + click |
| Rename a layer | double-click |
| Turn a mask off and on | Shift + click on the mask |

The left mouse button on its own paints. The 2D view navigates the same way, with Alt + left
mouse rotating it.

**Pen tablets on Linux:** pressure works on Wayland (GNOME, KDE) and on X11.

**Linux:** many desktops move windows with Alt + drag, which swallows the camera controls.
Switch the window manager to the Super key instead:

- GNOME: `gsettings set org.gnome.desktop.wm.preferences mouse-button-modifier '<Super>'`
- KDE Plasma: System Settings → Window Management → Window Behavior → Window Actions →
  Modifier key: Meta

## Architecture

Written in Rust on top of [wgpu](https://wgpu.rs) (Vulkan / DirectX 12 / Metal) and
[egui](https://github.com/emilk/egui).

| Crate | Purpose |
|---|---|
| `ink_core` | Data model: meshes, objects and texture sets, picking (BVH), brushes, strokes, the layer stack with masks and effects, undo, export templates; later vector paths |
| `ink_io` | Model import (ufbx, gltf, tobj), HDR environments, brush alphas and fill images, project files, texture export |
| `ink_render` | wgpu renderer: 3D viewport with image based lighting, 2D view, stroke rasterization and compositing, WGSL shaders |
| `ink_app` | Application window and UI (eframe / egui), pen tablet input |

CI builds, lints and tests every push on Windows and Linux.

## Credits

The built-in environment is [Studio Small 09](https://polyhaven.com/a/studio_small_09) by
Poly Haven, released under CC0.

## License

[GPL-3.0-or-later](https://www.gnu.org/licenses/gpl-3.0.html) — inkpainter3d and any
derivative of it stay free and open.
