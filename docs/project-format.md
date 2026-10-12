# Project file format (`.ink3d`)

An inkpainter3d project is one file that holds everything needed to continue working: the
mesh, the texture sets with their layer stacks, every brush stroke, the images of fill
layers and the brush alphas. It opens without the original files. Strokes are stored as
data, not as pixels, and are painted again when the project opens.

This page describes **version 1**. The code is `crates/ink_io/src/project.rs`.

## Container

A ZIP archive with these entries:

| Entry | Content | Compression |
|---|---|---|
| `project.json` | The structure: model, texture sets, layers, strokes, settings | Deflate |
| `mesh.bin` | The triangle mesh | Deflate |
| `dabs.bin` | The dabs (brush stamps) of all strokes | Deflate |
| `images/<n>.png`, `images/<n>.exr` | Images of fill layers and fill effects | Stored |
| `alphas/<n>.png` | Brush alphas used by strokes | Stored |

All binary numbers are little-endian; floats are IEEE 754 single precision.

## `project.json`

```json
{
  "format": "inkpainter3d project",
  "version": 1,
  "generator": "inkpainter3d 0.1.0",
  "name": "Cat",
  "source": "/models/cat.fbx",
  "normal_format": "directx",
  "last_id": 412,
  "mesh": { "vertices": 15230, "indices": 72018 },
  "objects": [{ "name": "Body", "visible": true }],
  "texture_sets": [ … ],
  "parts": [{ "object": 0, "texture_set": 0, "first_index": 0, "index_count": 36000 }],
  "images": [{ "file": "images/0.png", "name": "rust", "width": 2048, "height": 2048, "texels": "rgba8" }],
  "alphas": [{ "file": "alphas/0.png", "name": "dots", "width": 256, "height": 256 }],
  "view": { … },
  "export": { … }
}
```

- `format` and `version` come first in meaning: readers refuse other formats and versions
  newer than they know. Fields a reader doesn't know are ignored, so later versions of the
  same major format may add fields.
- `name` is the model's name (the mesh file's name without extension), `source` the mesh
  file the project was made from.
- `normal_format` (`"opengl"` or `"directx"`): the project's normal map format. It decides
  the normal map of the document channels export; layers are independent of it.
- `last_id`: the highest id handed out. Layers, paint layers, effects and strokes have ids,
  unique within the project (strokes only within their paint layer); new ones continue
  after `last_id`.
- `objects` and `texture_sets` are referred to by their index in these lists.
- `parts` cut the mesh's index list into contiguous ranges, in order, grouped by texture
  set: each range belongs to one object and one texture set. Together they cover the whole
  index list.
- `view` (camera and display settings) and `export` (the export window's settings) are
  optional; readers drop them if they don't understand them.

### Texture sets

```json
{ "name": "Skin", "visible": true, "resolution": 2048, "alpha_mode": "opaque", "layers": [ … ] }
```

`resolution` is a power of two; `alpha_mode` is `"opaque"`, `"test"` or `"blend"`. `layers`
lists the layer stack **bottom to top**.

### Layers

```json
{
  "id": 7, "name": "Rust", "visible": true,
  "blending": { "base_color": { "mode": "multiply", "opacity": 0.5 }, … },
  "content": { "fill": { … } },
  "mask": null
}
```

- `blending` maps channel keys to a blend mode and an opacity (0 to 1). Channels missing
  get the defaults of a new layer (Normal; Linear Dodge for height, Normal Map Detail for
  normals).
- Channel keys: `base_color`, `height`, `roughness`, `metallic`, `normal`, `emissive`,
  `opacity`.
- Blend mode keys: `normal`, `passthrough`, `disable`, `replace`, `multiply`, `divide`,
  `inverse_divide`, `darken`, `lighten`, `linear_dodge`, `subtract`, `inverse_subtract`,
  `difference`, `exclusion`, `signed_addition`, `overlay`, `screen`, `linear_burn`,
  `color_burn`, `color_dodge`, `soft_light`, `hard_light`, `vivid_light`, `linear_light`,
  `pin_light`, `tint`, `saturation`, `color`, `value`, `normal_map_combine`,
  `normal_map_detail`, `normal_map_inverse_detail`.

`content` is one of:

- `{ "paint": { "id": 8, "strokes": [ … ] } }`: a paint layer.
- `{ "fill": { "channels": ["base_color", "roughness"], "values": { … }, "images": { "base_color": 0 }, "normal_directx": true, "projection": { … } } }`:
  a fill layer. `channels` are the channels it sets; `values` the uniform values
  (`base_color`, `height`, `roughness`, `metallic`, `normal`, `emissive`, `opacity`; colors
  are sRGB, the normal is encoded like a normal map texel, 0.5 0.5 1 being flat); `images`
  maps channels to indices into the top-level `images`; `normal_directx` says whether the
  normal image is a DirectX normal map.
- `{ "folder": { "expanded": true, "layers": [ … ] } }`: a folder with its own layers,
  bottom to top.

`projection`: `{ "mode": "uv" | "triplanar", "scale": [1, 1], "rotation": 0, "offset": [0, 0], "hardness": 0.5 }`;
the rotation is in degrees, counter-clockwise.

### Masks and effects

```json
{ "enabled": true, "background": 0.0, "paint": { "id": 9, "strokes": [ … ] }, "effects": [ … ] }
```

`background` is 0 for a black mask, 1 for a white one; `paint` is painted directly into the
mask. Effects are listed bottom to top:

```json
{ "id": 10, "visible": true, "blend": "multiply", "opacity": 1.0, "kind": { "levels": { "input": [0, 1], "gamma": 1, "output": [0, 1] } } }
```

`kind` is `{ "paint": { … } }` (a paint layer), `{ "fill": { "value": 1.0, "image": null, "projection": { … } } }`
(a grayscale fill; the image's red channel is used) or `{ "levels": { … } }`.

### Strokes

```json
{
  "id": 21, "tool": "paint", "space": "surface", "alpha": "round",
  "hardness": 0.8, "opacity": 1.0, "backface_cos": 0.0,
  "material": { "channels": ["base_color", "roughness"], "values": { … }, "grayscale": 1.0 },
  "camera": { "view_proj": [ … 16 … ], "eye": [0, 1, 4], "forward": [0, 0, -1], "aspect": 1.6, "orthographic": false, "height": 0.73 },
  "hidden_objects": [], "hidden_sets": [],
  "first_dab": 0, "dab_count": 96
}
```

- `tool`: `"paint"` or `"erase"`. `space`: `"surface"` (painted in the 3D view; dabs in
  world space) or `"uv"` (painted in the 2D view; dabs in UV space, origin top left).
- `alpha`: `"round"`, `"square"` or `{ "image": n }`, an index into the top-level `alphas`.
- `backface_cos`: the cosine of the backface culling angle; -1 paints every surface.
- `material`: what the brush paints, as for fill layers; `grayscale` is the value painted
  into masks.
- `camera`: the view the stroke was painted from (texels hidden behind other surfaces get
  no paint); `view_proj` is a 4 × 4 matrix, column by column. `null` paints through
  everything.
- `hidden_objects`, `hidden_sets`: indices of objects and texture sets that were hidden;
  they got no paint and hid nothing.
- `first_dab`, `dab_count`: the stroke's dabs in `dabs.bin`.

## `mesh.bin`

For `mesh.vertices` = V and `mesh.indices` = I, one after the other:

| Data | Size |
|---|---|
| positions, x y z per vertex, in world space (meters, y up) | V × 12 bytes |
| normals, x y z | V × 12 bytes |
| UVs, u v, origin at the top left | V × 8 bytes |
| tangents, x y z and the bitangent sign w (MikkTSpace; the bitangent points up in the image) | V × 16 bytes |
| indices, u32, three per triangle | I × 4 bytes |

This is the mesh as inkpainter3d prepared it on import (triangulated, vertices split where
attributes differ), not the original file.

## `dabs.bin`

14 floats per dab, the dabs of each stroke one after the other:

| Floats | Meaning |
|---|---|
| 0–2 | position (world space, or u v 0 for strokes in UV space) |
| 3–5 | surface normal |
| 6–8 | direction of the top of the alpha |
| 9–11 | direction from the camera to the dab |
| 12 | radius (world units, or UV units) |
| 13 | flow (paint added by the dab, 0 to 1, with pen pressure applied) |

## Images

- `images/<n>.png`: `texels` `"rgba8"` or `"rgba16"`: 8 or 16 bits per component, RGBA, as
  loaded (sRGB-encoded where the image is a color).
- `images/<n>.exr`: `texels` `"rgba32f"`: linear float RGBA, from HDR and EXR files.
- `alphas/<n>.png`: 8-bit grayscale; white paints.

Images are stored losslessly, so they come back exactly.
