//! Texture export, as Painter's Export Textures window: output templates say which files an
//! export writes and how the composited channels are packed into them; the settings say
//! where, in which file format and with which padding.
//!
//! The renderer composites a texture set and hands over its texels as [`ExportTexel`]s, a
//! block at a time ([`ExportBlock`]); [`MapImage::pack`] turns them into the pixels of one
//! output map, and `ink_io` writes the files.

use crate::model::NormalFormat;

/// A texel of a texture set as the renderer composited it, with the defaults filled in
/// where no layer put anything. Colors are linear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportTexel {
    pub base_color: [f32; 3],
    pub opacity: f32,
    /// As painted: 0 is the surface, positive values rise.
    pub height: f32,
    pub roughness: f32,
    pub metallic: f32,
    /// Unit tangent-space normal, OpenGL style (y up), with the detail of the height
    /// channel combined in, as Painter's "Normal OpenGL" converted map.
    pub normal: [f32; 3],
    pub emissive: [f32; 3],
    /// Distance in texels to the nearest texel inside a UV island of the texture set; 0
    /// inside. Texels outside hold the values of that nearest texel (infinite dilation).
    pub island_distance: f32,
}

impl ExportTexel {
    /// What an empty layer stack shows: Painter's shader defaults (the compositor's
    /// `background`).
    pub const EMPTY: Self = Self {
        base_color: [0.5; 3],
        opacity: 1.0,
        height: 0.0,
        roughness: 0.3,
        metallic: 0.0,
        normal: [0.0, 0.0, 1.0],
        emissive: [0.0; 3],
        island_distance: 0.0,
    };
}

/// A rectangle of composited texels, row by row.
#[derive(Clone, Debug, Default)]
pub struct ExportBlock {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub texels: Vec<ExportTexel>,
}

/// A grayscale or color image an output map takes its values from: a channel of the
/// texture set or a map converted from the channels (Painter's "Converted maps").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    BaseColor,
    /// Exported with 0.5 as the surface, as Painter does.
    Height,
    Roughness,
    Metallic,
    Emissive,
    Opacity,
    /// The normal with the height's detail, in the project's normal map format.
    Normal,
    NormalOpenGl,
    NormalDirectX,
    /// 1 - roughness (Painter's "Glossiness", Unity's smoothness).
    Glossiness,
    /// Painter's "Mixed AO": the ambient occlusion channel times the baked ambient
    /// occlusion. White until baking exists.
    AmbientOcclusion,
    White,
    Black,
}

impl Source {
    /// The value at a texel: RGB for colors and normals, the same value three times for
    /// grayscale sources. Colors are linear; normals are encoded (0.5, 0.5, 1 is flat).
    pub fn value(self, texel: &ExportTexel, format: NormalFormat) -> [f32; 3] {
        let gray = |v: f32| [v; 3];
        let encode = |n: [f32; 3], directx: bool| {
            let y = if directx { -n[1] } else { n[1] };
            [n[0] * 0.5 + 0.5, y * 0.5 + 0.5, n[2] * 0.5 + 0.5]
        };
        match self {
            Self::BaseColor => texel.base_color,
            Self::Height => gray(texel.height * 0.5 + 0.5),
            Self::Roughness => gray(texel.roughness),
            Self::Metallic => gray(texel.metallic),
            Self::Emissive => texel.emissive,
            Self::Opacity => gray(texel.opacity),
            Self::Normal => encode(texel.normal, format == NormalFormat::DirectX),
            Self::NormalOpenGl => encode(texel.normal, false),
            Self::NormalDirectX => encode(texel.normal, true),
            Self::Glossiness => gray(1.0 - texel.roughness),
            Self::AmbientOcclusion | Self::White => gray(1.0),
            Self::Black => gray(0.0),
        }
    }

    /// Colors are stored sRGB-encoded in 8- and 16-bit files; everything else linear.
    pub fn is_color(self) -> bool {
        matches!(self, Self::BaseColor | Self::Emissive)
    }

    /// The channel's value where nothing is painted, for "Dilation + default background
    /// color".
    pub fn background(self, format: NormalFormat) -> [f32; 3] {
        self.value(&ExportTexel::EMPTY, format)
    }
}

/// How an output map's components are filled (Painter's map types Gray, RGB, RGB+A and
/// R+G+B+A).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Gray(Source),
    Rgb(Source),
    /// RGB from a color, alpha from a grayscale source.
    RgbA(Source, Source),
    /// Each component from a grayscale source.
    Components([Source; 4]),
    ComponentsRgb([Source; 3]),
}

impl Layout {
    /// Number of components: 1 (gray), 3 (RGB) or 4 (RGBA).
    pub fn components(self) -> usize {
        match self {
            Self::Gray(_) => 1,
            Self::Rgb(_) | Self::ComponentsRgb(_) => 3,
            Self::RgbA(..) | Self::Components(_) => 4,
        }
    }

    /// The source of a component and which of its three values it takes.
    fn source(self, component: usize) -> (Source, usize) {
        match self {
            Self::Gray(s) => (s, 0),
            Self::Rgb(s) => (s, component),
            Self::RgbA(s, _) if component < 3 => (s, component),
            Self::RgbA(_, a) => (a, 0),
            Self::Components(s) => (s[component], 0),
            Self::ComponentsRgb(s) => (s[component], 0),
        }
    }
}

/// One file an output template writes per texture set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputMap {
    /// The file name without extension: `$mesh` is replaced by the mesh name,
    /// `$textureSet` by the texture set's name and `$project` by the project's.
    pub name: &'static str,
    pub layout: Layout,
}

/// A set of output maps for a target application, as Painter's output templates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    /// A stable name for project files.
    pub key: &'static str,
    pub label: &'static str,
    pub maps: &'static [OutputMap],
}

const fn map(name: &'static str, layout: Layout) -> OutputMap {
    OutputMap { name, layout }
}

use Layout::{Components, ComponentsRgb, Gray, Rgb, RgbA};
use Source::*;

/// The shipped templates. Normal maps are OpenGL style for Blender, Unity, Godot and glTF,
/// DirectX for Unreal; the document channels follow the project's format.
pub const TEMPLATES: &[Template] = &[
    Template {
        key: "document_channels_no_alpha",
        label: "Document channels + Normal + AO (No Alpha)",
        maps: &[
            map("$mesh_$textureSet_BaseColor", Rgb(BaseColor)),
            map("$mesh_$textureSet_Height", Gray(Height)),
            map("$mesh_$textureSet_Roughness", Gray(Roughness)),
            map("$mesh_$textureSet_Metallic", Gray(Metallic)),
            map("$mesh_$textureSet_Normal", Rgb(Normal)),
            map("$mesh_$textureSet_Emissive", Rgb(Emissive)),
            map("$mesh_$textureSet_Opacity", Gray(Opacity)),
            map("$mesh_$textureSet_AO", Gray(AmbientOcclusion)),
        ],
    },
    Template {
        key: "document_channels_with_alpha",
        label: "Document channels + Normal + AO (With Alpha)",
        maps: &[
            map("$mesh_$textureSet_BaseColor", RgbA(BaseColor, Opacity)),
            map("$mesh_$textureSet_Height", Gray(Height)),
            map("$mesh_$textureSet_Roughness", Gray(Roughness)),
            map("$mesh_$textureSet_Metallic", Gray(Metallic)),
            map("$mesh_$textureSet_Normal", Rgb(Normal)),
            map("$mesh_$textureSet_Emissive", Rgb(Emissive)),
            map("$mesh_$textureSet_AO", Gray(AmbientOcclusion)),
        ],
    },
    Template {
        key: "pbr_metallic_roughness",
        label: "PBR Metallic Roughness",
        maps: &[
            map("$mesh_$textureSet_BaseColor", Rgb(BaseColor)),
            map("$mesh_$textureSet_Roughness", Gray(Roughness)),
            map("$mesh_$textureSet_Metallic", Gray(Metallic)),
            map("$mesh_$textureSet_Normal", Rgb(NormalOpenGl)),
            map("$mesh_$textureSet_Height", Gray(Height)),
            map("$mesh_$textureSet_Emissive", Rgb(Emissive)),
        ],
    },
    // As Painter's: separate maps named so that Blender's Node Wrangler connects them
    // (Ctrl+Shift+T); the displacement is the height, 0.5 at the surface.
    Template {
        key: "blender_principled_bsdf",
        label: "Blender (Principled BSDF)",
        maps: &[
            map("$mesh_$textureSet_BaseColor", Rgb(BaseColor)),
            map("$mesh_$textureSet_Metallic", Gray(Metallic)),
            map("$mesh_$textureSet_Roughness", Gray(Roughness)),
            map("$mesh_$textureSet_Normal", Rgb(NormalOpenGl)),
            map("$mesh_$textureSet_Emission", Rgb(Emissive)),
            map("$mesh_$textureSet_Alpha", Gray(Opacity)),
            map("$mesh_$textureSet_Displacement", Gray(Height)),
        ],
    },
    Template {
        key: "unity_hdrp_metallic",
        label: "Unity HD Render Pipeline (Metallic Standard)",
        maps: &[
            map("$mesh_$textureSet_BaseMap", RgbA(BaseColor, Opacity)),
            // Metallic, AO, detail mask (white: details everywhere, as Unity assumes
            // without a mask map), smoothness.
            map(
                "$mesh_$textureSet_MaskMap",
                Components([Metallic, AmbientOcclusion, White, Glossiness]),
            ),
            map("$mesh_$textureSet_Normal", Rgb(NormalOpenGl)),
            map("$mesh_$textureSet_Emissive", Rgb(Emissive)),
        ],
    },
    Template {
        key: "unity_urp_metallic",
        label: "Unity Universal Render Pipeline (Metallic Standard)",
        maps: &[
            map("$mesh_$textureSet_BaseMap", RgbA(BaseColor, Opacity)),
            map(
                "$mesh_$textureSet_MetallicSmoothness",
                RgbA(Metallic, Glossiness),
            ),
            map("$mesh_$textureSet_Normal", Rgb(NormalOpenGl)),
            map("$mesh_$textureSet_Occlusion", Gray(AmbientOcclusion)),
            map("$mesh_$textureSet_Emission", Rgb(Emissive)),
        ],
    },
    Template {
        key: "unity_builtin_metallic",
        label: "Unity Built-in (Standard Metallic)",
        maps: &[
            map(
                "$mesh_$textureSet_AlbedoTransparency",
                RgbA(BaseColor, Opacity),
            ),
            map(
                "$mesh_$textureSet_MetallicSmoothness",
                RgbA(Metallic, Glossiness),
            ),
            map("$mesh_$textureSet_Normal", Rgb(NormalOpenGl)),
            map("$mesh_$textureSet_Occlusion", Gray(AmbientOcclusion)),
            map("$mesh_$textureSet_Emission", Rgb(Emissive)),
        ],
    },
    Template {
        key: "godot_orm",
        label: "Godot (ORMMaterial3D)",
        maps: &[
            map("$mesh_$textureSet_Albedo", RgbA(BaseColor, Opacity)),
            map(
                "$mesh_$textureSet_ORM",
                ComponentsRgb([AmbientOcclusion, Roughness, Metallic]),
            ),
            map("$mesh_$textureSet_Normal", Rgb(NormalOpenGl)),
            map("$mesh_$textureSet_Emission", Rgb(Emissive)),
            map("$mesh_$textureSet_Height", Gray(Height)),
        ],
    },
    Template {
        key: "gltf_pbr_metal_roughness",
        label: "glTF PBR Metal Roughness",
        maps: &[
            map("$mesh_$textureSet_baseColor", RgbA(BaseColor, Opacity)),
            map(
                "$mesh_$textureSet_occlusionRoughnessMetallic",
                ComponentsRgb([AmbientOcclusion, Roughness, Metallic]),
            ),
            map("$mesh_$textureSet_normal", Rgb(NormalOpenGl)),
            map("$mesh_$textureSet_emissive", Rgb(Emissive)),
        ],
    },
    Template {
        key: "unreal_engine_packed",
        label: "Unreal Engine (Packed)",
        maps: &[
            map("$mesh_$textureSet_BaseColor", RgbA(BaseColor, Opacity)),
            map(
                "$mesh_$textureSet_OcclusionRoughnessMetallic",
                ComponentsRgb([AmbientOcclusion, Roughness, Metallic]),
            ),
            map("$mesh_$textureSet_Normal", Rgb(NormalDirectX)),
            map("$mesh_$textureSet_Emissive", Rgb(Emissive)),
        ],
    },
];

impl Template {
    pub fn from_key(key: &str) -> Option<&'static Self> {
        TEMPLATES.iter().find(|t| t.key == key)
    }
}

/// File formats of the export (Painter's File type).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FileFormat {
    #[default]
    Png,
    Targa,
    Jpeg,
    Tiff,
    OpenExr,
}

impl FileFormat {
    pub const ALL: [Self; 5] = [
        Self::Png,
        Self::Targa,
        Self::Jpeg,
        Self::Tiff,
        Self::OpenExr,
    ];

    /// As in Painter's File type list.
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Targa => "targa",
            Self::Jpeg => "jpeg",
            Self::Tiff => "tiff",
            Self::OpenExr => "exr",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Targa => "tga",
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
            Self::OpenExr => "exr",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.label() == key)
    }

    /// The bit depths the format can store, the first being the default.
    pub fn bit_depths(self) -> &'static [BitDepth] {
        use BitDepth::*;
        match self {
            Self::Png | Self::Tiff => &[Eight, EightDithered, Sixteen],
            Self::Targa | Self::Jpeg => &[Eight, EightDithered],
            Self::OpenExr => &[Float16, Float32],
        }
    }

    /// Whether files of this format can have an alpha channel.
    pub fn has_alpha(self) -> bool {
        self != Self::Jpeg
    }
}

/// Bits per component of the exported files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BitDepth {
    #[default]
    Eight,
    /// 8 bits with noise against banding in gradients, as Painter's "8 + dithering".
    EightDithered,
    Sixteen,
    Float16,
    Float32,
}

impl BitDepth {
    pub const ALL: [Self; 5] = [
        Self::Eight,
        Self::EightDithered,
        Self::Sixteen,
        Self::Float16,
        Self::Float32,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Eight => "8 bits",
            Self::EightDithered => "8 bits + dithering",
            Self::Sixteen => "16 bits",
            Self::Float16 => "16 bits (float)",
            Self::Float32 => "32 bits (float)",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Eight => "8",
            Self::EightDithered => "8_dithered",
            Self::Sixteen => "16",
            Self::Float16 => "16f",
            Self::Float32 => "32f",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.key() == key)
    }

    pub fn is_float(self) -> bool {
        matches!(self, Self::Float16 | Self::Float32)
    }
}

/// How the texels outside the UV islands are filled (Painter's Padding).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Padding {
    /// "No padding (passthrough)": the texture as the texture set holds it. Ours is
    /// dilated to the borders already.
    Passthrough,
    /// "Dilation infinite": island borders stretch until they meet.
    #[default]
    Infinite,
    /// "Dilation + transparent": stretched by the dilation distance, transparent beyond.
    Transparent,
    /// "Dilation + default background color": stretched by the dilation distance, the
    /// channels' defaults beyond.
    Background,
}

impl Padding {
    pub const ALL: [Self; 4] = [
        Self::Passthrough,
        Self::Infinite,
        Self::Transparent,
        Self::Background,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Passthrough => "No padding (passthrough)",
            Self::Infinite => "Dilation infinite",
            Self::Transparent => "Dilation + transparent",
            Self::Background => "Dilation + default background color",
        }
    }

    /// Painter's names in its export configurations.
    pub fn key(self) -> &'static str {
        match self {
            Self::Passthrough => "passthrough",
            Self::Infinite => "infinite",
            Self::Transparent => "transparent",
            Self::Background => "color",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    /// Whether the dilation distance applies.
    pub fn has_distance(self) -> bool {
        matches!(self, Self::Transparent | Self::Background)
    }
}

/// The choices of the export window that are kept with the project.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSettings {
    /// Where the files go; empty until chosen.
    pub directory: String,
    pub template: &'static Template,
    pub format: FileFormat,
    pub bit_depth: BitDepth,
    pub padding: Padding,
    /// In texels, for the paddings that have one.
    pub dilation: u32,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            directory: String::new(),
            template: &TEMPLATES[0],
            format: FileFormat::Png,
            bit_depth: BitDepth::Eight,
            padding: Padding::Infinite,
            dilation: 16,
        }
    }
}

impl ExportSettings {
    /// Largest dilation distance offered.
    pub const MAX_DILATION: u32 = 256;

    /// Whether files get an alpha channel: maps with alpha, and with transparent padding
    /// every map, if the format can store it.
    pub fn components(&self, map: &OutputMap) -> usize {
        let n = map.layout.components();
        if !self.format.has_alpha() {
            return n.min(3);
        }
        if self.padding == Padding::Transparent {
            match n {
                1 => 2,
                3 => 4,
                n => n,
            }
        } else {
            n
        }
    }
}

/// Replaces `$mesh`, `$textureSet` and `$project` in a map name and makes the result a
/// valid file name on Windows and Linux.
pub fn file_name(pattern: &str, mesh: &str, texture_set: &str, project: &str) -> String {
    let name = pattern
        .replace("$textureSet", texture_set)
        .replace("$mesh", mesh)
        .replace("$project", project);
    let name: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    // Windows drops trailing dots and spaces.
    let name = name.trim_end_matches(['.', ' ']);
    if name.is_empty() {
        "texture".to_owned()
    } else {
        name.to_owned()
    }
}

/// Pixels of an output map, row by row from the top, `components` values per pixel.
#[derive(Clone, Debug, PartialEq)]
pub enum MapPixels {
    U8(Vec<u8>),
    U16(Vec<u16>),
    /// Float files: 16- or 32-bit float components.
    F32(Vec<f32>),
}

/// An output map of one texture set while it is being packed.
#[derive(Clone, Debug, PartialEq)]
pub struct MapImage {
    pub width: u32,
    pub height: u32,
    /// 1 (gray), 2 (gray and alpha), 3 (RGB) or 4 (RGBA).
    pub components: usize,
    pub pixels: MapPixels,
}

impl MapImage {
    pub fn new(size: u32, components: usize, bit_depth: BitDepth) -> Self {
        let len = size as usize * size as usize * components;
        let pixels = match bit_depth {
            BitDepth::Eight | BitDepth::EightDithered => MapPixels::U8(vec![0; len]),
            BitDepth::Sixteen => MapPixels::U16(vec![0; len]),
            BitDepth::Float16 | BitDepth::Float32 => MapPixels::F32(vec![0.0; len]),
        };
        Self {
            width: size,
            height: size,
            components,
            pixels,
        }
    }

    /// Packs a block of composited texels into the map's pixels.
    pub fn pack(
        &mut self,
        block: &ExportBlock,
        map: &OutputMap,
        settings: &ExportSettings,
        format: NormalFormat,
    ) {
        let dithered = settings.bit_depth == BitDepth::EightDithered;
        let float = settings.bit_depth.is_float();
        let layout = map.layout;
        let colors = layout.components();
        let components = self.components;
        let width = self.width as usize;
        for row in 0..block.height {
            let y = block.y + row;
            if y >= self.height {
                break;
            }
            for column in 0..block.width {
                let x = block.x + column;
                if x >= self.width {
                    break;
                }
                let texel = &block.texels[(row * block.width + column) as usize];
                let outside = settings.padding.has_distance()
                    && texel.island_distance > settings.dilation as f32;
                let base = (y as usize * width + x as usize) * components;
                for c in 0..components {
                    let value = if c >= colors {
                        // The alpha channel transparent padding adds.
                        if outside { 0.0 } else { 1.0 }
                    } else if outside && settings.padding == Padding::Transparent {
                        0.0
                    } else {
                        let (source, index) = layout.source(c);
                        let v = if outside {
                            source.background(format)[index]
                        } else {
                            source.value(texel, format)[index]
                        };
                        // Color files in 8 and 16 bits are sRGB, float files linear.
                        if source.is_color() && !float {
                            linear_to_srgb(v)
                        } else {
                            v
                        }
                    };
                    match &mut self.pixels {
                        MapPixels::U8(p) => {
                            let noise = if dithered { dither(x, y, c) } else { 0.0 };
                            p[base + c] = quantize(value, 255.0, noise) as u8;
                        }
                        MapPixels::U16(p) => p[base + c] = quantize(value, 65535.0, 0.0) as u16,
                        MapPixels::F32(p) => p[base + c] = value,
                    }
                }
            }
        }
    }
}

fn quantize(value: f32, max: f32, noise: f32) -> f32 {
    (value.clamp(0.0, 1.0) * max + noise)
        .round()
        .clamp(0.0, max)
}

/// Triangular noise of up to one step either way, the same for every export.
fn dither(x: u32, y: u32, component: usize) -> f32 {
    let hash = |n: u32| {
        let mut h = n.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
        h ^= h >> 16;
        h = h.wrapping_mul(0x7feb_352d);
        h ^= h >> 15;
        h = h.wrapping_mul(0x846c_a68b);
        h ^= h >> 16;
        h as f32 / u32::MAX as f32
    };
    let seed = x.wrapping_mul(73_856_093)
        ^ y.wrapping_mul(19_349_663)
        ^ (component as u32).wrapping_mul(83_492_791);
    hash(seed) + hash(seed ^ 0x5bd1_e995) - 1.0
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texel() -> ExportTexel {
        ExportTexel {
            base_color: [0.2, 0.5, 1.0],
            opacity: 0.25,
            height: 0.5,
            roughness: 0.2,
            metallic: 1.0,
            normal: [0.6, 0.0, 0.8],
            emissive: [0.0; 3],
            island_distance: 0.0,
        }
    }

    /// Packs two texels into a 2 × 1 map.
    fn pack(map: &OutputMap, settings: &ExportSettings, texels: &[ExportTexel]) -> MapImage {
        let mut image = MapImage::new(2, settings.components(map), settings.bit_depth);
        image.height = 1;
        match &mut image.pixels {
            MapPixels::U8(p) => p.truncate(p.len() / 2),
            MapPixels::U16(p) => p.truncate(p.len() / 2),
            MapPixels::F32(p) => p.truncate(p.len() / 2),
        }
        let block = ExportBlock {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            texels: texels.to_vec(),
        };
        image.pack(&block, map, settings, NormalFormat::OpenGl);
        image
    }

    fn template(key: &str) -> &'static Template {
        Template::from_key(key).unwrap()
    }

    #[test]
    fn templates_have_unique_keys_and_names() {
        for (i, t) in TEMPLATES.iter().enumerate() {
            assert!(
                TEMPLATES[..i]
                    .iter()
                    .all(|o| o.key != t.key && o.label != t.label)
            );
            for (j, m) in t.maps.iter().enumerate() {
                assert!(t.maps[..j].iter().all(|o| o.name != m.name), "{}", m.name);
            }
        }
    }

    #[test]
    fn unity_mask_map_packs_metallic_ao_detail_and_smoothness() {
        let t = template("unity_hdrp_metallic");
        let mask = t.maps.iter().find(|m| m.name.ends_with("MaskMap")).unwrap();
        let image = pack(mask, &ExportSettings::default(), &[texel(), texel()]);
        let MapPixels::U8(p) = &image.pixels else {
            panic!()
        };
        assert_eq!(&p[..4], &[255, 255, 255, 204], "smoothness = 1 - roughness");
    }

    #[test]
    fn colors_are_srgb_in_integer_files_and_linear_in_float_files() {
        let t = template("unity_urp_metallic");
        let base = &t.maps[0];
        let image = pack(base, &ExportSettings::default(), &[texel(), texel()]);
        let MapPixels::U8(p) = &image.pixels else {
            panic!()
        };
        // 0.2 linear is 124 in sRGB; alpha (opacity) stays linear.
        assert_eq!(&p[..4], &[124, 188, 255, 64]);
        let settings = ExportSettings {
            format: FileFormat::OpenExr,
            bit_depth: BitDepth::Float32,
            ..Default::default()
        };
        let image = pack(base, &settings, &[texel(), texel()]);
        let MapPixels::F32(p) = &image.pixels else {
            panic!()
        };
        assert_eq!(&p[..4], &[0.2, 0.5, 1.0, 0.25]);
    }

    #[test]
    fn normals_follow_the_requested_format() {
        let up = ExportTexel {
            normal: [0.0, 0.6, 0.8],
            ..texel()
        };
        let gl = Source::NormalOpenGl.value(&up, NormalFormat::DirectX);
        let dx = Source::NormalDirectX.value(&up, NormalFormat::OpenGl);
        assert!((gl[1] - 0.8).abs() < 1e-6 && (dx[1] - 0.2).abs() < 1e-6);
        assert_eq!(Source::Normal.value(&up, NormalFormat::DirectX), dx);
        assert_eq!(Source::Normal.value(&up, NormalFormat::OpenGl), gl);
        // Height 0 is mid gray.
        assert_eq!(Source::Height.background(NormalFormat::OpenGl), [0.5; 3]);
    }

    #[test]
    fn padding_fills_beyond_the_dilation_distance() {
        let far = ExportTexel {
            island_distance: 20.0,
            ..texel()
        };
        let near = ExportTexel {
            island_distance: 3.0,
            ..texel()
        };
        let rough = Gray(Roughness);
        let roughness = map("r", rough);
        for (padding, expected) in [
            (Padding::Infinite, [51, 51]),
            (Padding::Passthrough, [51, 51]),
            (Padding::Background, [51, 77]),
        ] {
            let settings = ExportSettings {
                padding,
                dilation: 16,
                ..Default::default()
            };
            let image = pack(&roughness, &settings, &[near, far]);
            assert_eq!(
                image.pixels,
                MapPixels::U8(expected.to_vec()),
                "{padding:?}"
            );
        }
        // Transparent padding adds an alpha channel to maps without one.
        let settings = ExportSettings {
            padding: Padding::Transparent,
            dilation: 16,
            ..Default::default()
        };
        let image = pack(&roughness, &settings, &[near, far]);
        assert_eq!(image.components, 2);
        assert_eq!(image.pixels, MapPixels::U8(vec![51, 255, 0, 0]));
        // JPEG has no alpha channel.
        let jpeg = ExportSettings {
            format: FileFormat::Jpeg,
            ..settings
        };
        assert_eq!(jpeg.components(&roughness), 1);
    }

    #[test]
    fn dithering_stays_within_a_step() {
        let settings = ExportSettings {
            bit_depth: BitDepth::EightDithered,
            ..Default::default()
        };
        let gray = map("g", Gray(Roughness));
        let mut image = MapImage::new(64, 1, settings.bit_depth);
        let block = ExportBlock {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
            texels: vec![
                ExportTexel {
                    roughness: 100.4 / 255.0,
                    ..texel()
                };
                64 * 64
            ],
        };
        image.pack(&block, &gray, &settings, NormalFormat::OpenGl);
        let MapPixels::U8(p) = &image.pixels else {
            panic!()
        };
        assert!(p.iter().all(|&v| (99..=102).contains(&v)));
        let mean = p.iter().map(|&v| v as f32).sum::<f32>() / p.len() as f32;
        assert!((mean - 100.4).abs() < 0.2, "{mean}");
    }

    #[test]
    fn file_names_are_valid_everywhere() {
        assert_eq!(
            file_name("$mesh_$textureSet_BaseColor", "Cat", "Fur/Body", "p"),
            "Cat_Fur_Body_BaseColor"
        );
        assert_eq!(file_name("$textureSet", "", "a:b. ", ""), "a_b");
        assert_eq!(file_name("$textureSet", "", "", ""), "texture");
    }

    #[test]
    fn bit_depths_suit_their_formats() {
        for format in FileFormat::ALL {
            let depths = format.bit_depths();
            assert!(!depths.is_empty());
            assert_eq!(
                depths.iter().all(|d| d.is_float()),
                format == FileFormat::OpenExr
            );
        }
    }
}
