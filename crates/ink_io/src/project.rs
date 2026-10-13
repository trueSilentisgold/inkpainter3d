//! Project files (`.ink3d`): everything needed to continue working on a model, in one file
//! that opens without the original mesh or images. A ZIP archive with the structure as JSON
//! and the bulk data in binary files; `docs/project-format.md` describes it.
//!
//! Strokes are stored as data, not as pixels: opening a project paints them again.

use anyhow::{Context, Result, bail, ensure};
use glam::{Mat4, Vec2, Vec3, Vec4};
use ink_core::export::{BitDepth, ExportSettings, FileFormat, Padding, Template};
use ink_core::{
    Aabb, Alignment, Alpha, AlphaImage, AlphaMode, BakeSettings, BlendMode, Blending,
    BrushMaterial, Channel, ChannelBlend, ChannelSet, ChannelValues, Content, Dab, Effect,
    EffectKind, Fill, FillTarget, Folder, Generator, GeneratorKind, GrayFill, IdSource, Ids, Layer,
    LayerStack, Levels, Mask, Matching, Mesh, MeshMap, MeshMaps, MeshObject, Model, NormalFormat,
    PaintLayer, Part, Projection, ProjectionMode, Stroke, StrokeCamera, StrokeSpace, StrokeStencil,
    Texels, TextureImage, TextureSet, Tool,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// File extension of project files.
pub const PROJECT_EXTENSION: &str = "ink3d";

const FORMAT: &str = "inkpainter3d project";
/// Raised when older programs can no longer read the files.
const VERSION: u32 = 1;

const PROJECT_JSON: &str = "project.json";
const MESH_BIN: &str = "mesh.bin";
const DABS_BIN: &str = "dabs.bin";
/// f32 values per dab in `dabs.bin`.
const DAB_FLOATS: usize = 14;

/// Largest texture set resolution a project may hold.
const MAX_RESOLUTION: u32 = 8192;
/// Highest id a project may hold: new ids continue after it.
const MAX_ID: u64 = u64::MAX / 2;

/// A model with everything around it that a project file keeps.
#[derive(Clone, Debug)]
pub struct Project {
    pub model: Model,
    /// The mesh file the project was made from, for re-importing it later.
    pub source: Option<String>,
    /// How the views looked when the project was saved.
    pub view: Option<ViewState>,
    /// The export window's settings.
    pub export: Option<ExportSettings>,
    /// The baking window's settings.
    pub bake: Option<BakeSettings>,
}

/// Camera and display settings of the views.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    pub camera_target: [f32; 3],
    pub camera_yaw: f32,
    pub camera_pitch: f32,
    pub camera_distance: f32,
    pub camera_fov_y: f32,
    pub orthographic: bool,
    pub stylized: bool,
    /// Path of the environment panorama; None for the default one.
    pub environment: Option<String>,
    pub environment_rotation: f32,
    pub exposure: f32,
    pub background_opacity: f32,
    pub background_blur: f32,
    pub toon_bands: u32,
    pub outline: bool,
    pub outline_color: [f32; 3],
    pub outline_width: f32,
}

/// Encoded images of the last save, so that saving again does not encode them again.
/// Images never change; they are known by their ids.
#[derive(Clone, Debug, Default)]
pub struct ImageCache {
    textures: HashMap<u64, Encoded>,
    alphas: HashMap<u64, Encoded>,
}

#[derive(Clone, Debug)]
pub(crate) struct Encoded {
    pub(crate) bytes: Arc<Vec<u8>>,
}

/// Writes the project to `path`. The file is replaced only once the new one is complete,
/// so a failed save leaves the old file as it was.
pub fn save(path: &Path, project: &Project, cache: &mut ImageCache) -> Result<()> {
    let mut temp_name = path.file_name().unwrap_or_default().to_owned();
    temp_name.push(".saving");
    let temp = path.with_file_name(temp_name);
    let result = (|| -> Result<()> {
        let file = std::fs::File::create(&temp)
            .with_context(|| format!("Failed to create {}", temp.display()))?;
        let mut writer = std::io::BufWriter::new(file);
        write(&mut writer, project, cache)?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
            .with_context(|| format!("Failed to replace {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.with_context(|| format!("Failed to save {}", path.display()))
}

/// Reads a project file. The cache holds its images as stored, for saving them again.
pub fn load(path: &Path) -> Result<(Project, ImageCache)> {
    let file =
        std::fs::File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    read(std::io::BufReader::new(file))
        .with_context(|| format!("Failed to read {}", path.display()))
}

/// Writes a project as a ZIP archive.
pub fn write<W: Write + Seek>(writer: W, project: &Project, cache: &mut ImageCache) -> Result<()> {
    let mut saver = Saver::default();
    let file = saver.project(project)?;
    let mut zip = zip::ZipWriter::new(writer);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(1));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let large = |len: usize| len as u64 >= u32::MAX as u64 / 2;

    let json = serde_json::to_vec(&file)?;
    zip.start_file(PROJECT_JSON, deflated)?;
    zip.write_all(&json)?;
    let mesh = mesh_bytes(&project.model.mesh);
    zip.start_file(MESH_BIN, deflated.large_file(large(mesh.len())))?;
    zip.write_all(&mesh)?;
    zip.start_file(DABS_BIN, deflated.large_file(large(saver.dabs.len())))?;
    zip.write_all(&saver.dabs)?;

    let mut used = ImageCache::default();
    for (image, entry) in saver.textures.iter().zip(&file.images) {
        let encoded = match cache.textures.get(&image.id) {
            Some(e) => e.clone(),
            None => encode_texture(image)?,
        };
        zip.start_file(&entry.file, stored.large_file(large(encoded.bytes.len())))?;
        zip.write_all(&encoded.bytes)?;
        used.textures.insert(image.id, encoded);
    }
    for (alpha, entry) in saver.alphas.iter().zip(&file.alphas) {
        let encoded = match cache.alphas.get(&alpha.id) {
            Some(e) => e.clone(),
            None => encode_alpha(alpha)?,
        };
        zip.start_file(&entry.file, stored)?;
        zip.write_all(&encoded.bytes)?;
        used.alphas.insert(alpha.id, encoded);
    }
    zip.finish()?;
    // Forget images the project no longer uses.
    *cache = used;
    Ok(())
}

/// Reads a project from a ZIP archive.
pub fn read<R: Read + Seek>(reader: R) -> Result<(Project, ImageCache)> {
    let mut zip = zip::ZipArchive::new(reader).context("Not a project file")?;
    let json = read_entry(&mut zip, PROJECT_JSON, 1 << 30)?;
    let header: Header = serde_json::from_slice(&json).context("Not a project file")?;
    ensure!(header.format == FORMAT, "Not an inkpainter3d project");
    ensure!(
        header.version <= VERSION,
        "The project was saved by a newer version of inkpainter3d (format {}; this version reads up to {VERSION})",
        header.version
    );
    let file: FileProject = serde_json::from_slice(&json).context("The project is damaged")?;

    let vertices = file.mesh.vertices as usize;
    let indices = file.mesh.indices as usize;
    let mesh_len = vertices
        .checked_mul(VERTEX_BYTES)
        .and_then(|v| v.checked_add(indices.checked_mul(4)?))
        .context("The mesh is too large")?;
    let mesh_data = read_entry(&mut zip, MESH_BIN, mesh_len as u64)?;
    let mesh = read_mesh(&mesh_data, vertices, indices)?;
    // The strokes' dabs lie one after the other: their count says how big dabs.bin is.
    let dab_bytes = file
        .texture_sets
        .iter()
        .try_fold(0u64, |total, s| total.checked_add(dab_count(&s.layers)?))
        .and_then(|n| n.checked_mul(DAB_FLOATS as u64 * 4))
        .context("The project's strokes are damaged")?;
    let dabs = read_entry(&mut zip, DABS_BIN, dab_bytes)?;
    ensure!(
        dabs.len() as u64 == dab_bytes,
        "The project's strokes are damaged"
    );

    let mut cache = ImageCache::default();
    let mut textures = Vec::with_capacity(file.images.len());
    for entry in &file.images {
        check_size(entry.width, entry.height, crate::MAX_TEXTURE_SIZE)?;
        let bytes = read_entry(
            &mut zip,
            &entry.file,
            encoded_limit(entry.width, entry.height),
        )?;
        let image = decode_texture(entry, &bytes)
            .with_context(|| format!("Failed to read the image {}", entry.file))?;
        cache.textures.insert(
            image.id,
            Encoded {
                bytes: Arc::new(bytes),
            },
        );
        textures.push(Arc::new(image));
    }
    let mut alphas = Vec::with_capacity(file.alphas.len());
    for entry in &file.alphas {
        check_size(entry.width, entry.height, crate::MAX_ALPHA_SIZE)?;
        let bytes = read_entry(
            &mut zip,
            &entry.file,
            encoded_limit(entry.width, entry.height),
        )?;
        let alpha = decode_alpha(entry, &bytes)
            .with_context(|| format!("Failed to read the brush alpha {}", entry.file))?;
        cache.alphas.insert(
            alpha.id,
            Encoded {
                bytes: Arc::new(bytes),
            },
        );
        alphas.push(Arc::new(alpha));
    }

    let highest = file
        .texture_sets
        .iter()
        .map(|s| highest_id(&s.layers))
        .max()
        .unwrap_or(0)
        .max(file.last_id);
    ensure!(highest < MAX_ID, "The project's ids are damaged");
    let loader = Loader {
        dabs: &dabs,
        next_dab: 0,
        textures: &textures,
        alphas: &alphas,
        ids: HashSet::new(),
        next_id: highest + 1,
    };
    let project = loader.project(file, mesh)?;
    Ok((project, cache))
}

fn read_entry<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>> {
    let entry = zip
        .by_name(name)
        .with_context(|| format!("The project has no {name}"))?;
    ensure!(entry.size() <= limit, "{name} is too large");
    let mut bytes = Vec::with_capacity(entry.size().min(1 << 28) as usize);
    entry.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "{name} is too large");
    Ok(bytes)
}

// ---- File structure (project.json) ----

#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

#[derive(Serialize, Deserialize)]
struct FileProject {
    format: String,
    version: u32,
    #[serde(default)]
    generator: String,
    name: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    normal_format: FileNormalFormat,
    last_id: u64,
    mesh: FileMesh,
    objects: Vec<FileObject>,
    texture_sets: Vec<FileTextureSet>,
    parts: Vec<FilePart>,
    #[serde(default)]
    images: Vec<FileImage>,
    #[serde(default)]
    alphas: Vec<FileAlpha>,
    /// Settings that were not understood are dropped rather than refusing the project.
    #[serde(default, deserialize_with = "lenient")]
    view: Option<ViewState>,
    #[serde(default, deserialize_with = "lenient")]
    export: Option<FileExport>,
    #[serde(default, deserialize_with = "lenient")]
    bake: Option<FileBake>,
}

fn lenient<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FileNormalFormat {
    #[serde(rename = "opengl")]
    OpenGl,
    #[default]
    #[serde(rename = "directx")]
    DirectX,
}

#[derive(Serialize, Deserialize)]
struct FileMesh {
    vertices: u32,
    indices: u32,
}

#[derive(Serialize, Deserialize)]
struct FileObject {
    name: String,
    #[serde(default = "yes")]
    visible: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct FileTextureSet {
    name: String,
    #[serde(default = "yes")]
    visible: bool,
    resolution: u32,
    #[serde(default)]
    alpha_mode: FileAlphaMode,
    layers: Vec<FileLayer>,
    /// Baked mesh maps.
    #[serde(default)]
    mesh_maps: Vec<FileMeshMap>,
}

#[derive(Serialize, Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum FileAlphaMode {
    #[default]
    Opaque,
    Test,
    Blend,
}

#[derive(Serialize, Deserialize)]
struct FilePart {
    object: usize,
    texture_set: usize,
    /// First index and index count in the mesh's index list.
    first_index: u32,
    index_count: u32,
}

#[derive(Serialize, Deserialize)]
struct FileLayer {
    id: u64,
    name: String,
    #[serde(default = "yes")]
    visible: bool,
    /// Blend mode and opacity by channel key; channels missing get the default.
    #[serde(default)]
    blending: BTreeMap<String, FileBlend>,
    content: FileContent,
    #[serde(default)]
    mask: Option<FileMask>,
}

#[derive(Serialize, Deserialize)]
struct FileBlend {
    mode: String,
    opacity: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FileContent {
    Paint(FilePaint),
    Fill(FileFill),
    Folder(FileFolder),
}

#[derive(Serialize, Deserialize)]
struct FilePaint {
    id: u64,
    strokes: Vec<FileStroke>,
}

#[derive(Serialize, Deserialize)]
struct FileFill {
    channels: Vec<String>,
    values: FileValues,
    /// Image index by channel key.
    #[serde(default)]
    images: BTreeMap<String, usize>,
    normal_directx: bool,
    projection: FileProjection,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct FileValues {
    base_color: [f32; 3],
    height: f32,
    roughness: f32,
    metallic: f32,
    normal: [f32; 3],
    emissive: [f32; 3],
    opacity: f32,
}

#[derive(Serialize, Deserialize)]
struct FileProjection {
    mode: FileProjectionMode,
    scale: [f32; 2],
    rotation: f32,
    offset: [f32; 2],
    hardness: f32,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum FileProjectionMode {
    Uv,
    #[serde(rename = "triplanar")]
    TriPlanar,
}

#[derive(Serialize, Deserialize)]
struct FileFolder {
    #[serde(default = "yes")]
    expanded: bool,
    layers: Vec<FileLayer>,
}

#[derive(Serialize, Deserialize)]
struct FileMask {
    #[serde(default = "yes")]
    enabled: bool,
    background: f32,
    paint: FilePaint,
    #[serde(default)]
    effects: Vec<FileEffect>,
}

#[derive(Serialize, Deserialize)]
struct FileEffect {
    id: u64,
    #[serde(default = "yes")]
    visible: bool,
    blend: String,
    opacity: f32,
    kind: FileEffectKind,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FileEffectKind {
    Paint(FilePaint),
    Fill(FileGrayFill),
    Levels(FileLevels),
    Generator(FileGenerator),
}

/// A generator by the keys of its kind and parameters, so the order of the parameters in the
/// program can change.
#[derive(Serialize, Deserialize)]
struct FileGenerator {
    kind: String,
    params: BTreeMap<String, f32>,
}

#[derive(Serialize, Deserialize)]
struct FileGrayFill {
    value: f32,
    #[serde(default)]
    image: Option<usize>,
    projection: FileProjection,
}

#[derive(Serialize, Deserialize)]
struct FileLevels {
    input: [f32; 2],
    gamma: f32,
    output: [f32; 2],
}

#[derive(Serialize, Deserialize)]
struct FileStroke {
    id: u64,
    tool: FileTool,
    space: FileSpace,
    alpha: FileAlphaShape,
    hardness: f32,
    /// `Alignment::key`; absent in older files (tangent wrap).
    #[serde(default)]
    alignment: Option<String>,
    opacity: f32,
    backface_cos: f32,
    material: FileBrushMaterial,
    #[serde(default)]
    camera: Option<FileCamera>,
    #[serde(default)]
    hidden_objects: Vec<usize>,
    #[serde(default)]
    hidden_sets: Vec<usize>,
    /// What a Polygon Fill stroke covers; such strokes have no dabs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fill: Option<FileStrokeFill>,
    /// The stencil the stroke was painted through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stencil: Option<FileStencil>,
    /// First dab and dab count in `dabs.bin`.
    first_dab: u64,
    dab_count: u64,
}

#[derive(Serialize, Deserialize)]
struct FileStencil {
    /// Index into the brush alphas.
    alpha: usize,
    affine: [f32; 6],
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FileStrokeFill {
    Triangles(Vec<u32>),
    Object(usize),
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum FileTool {
    Paint,
    Erase,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum FileSpace {
    Surface,
    Uv,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum FileAlphaShape {
    Round,
    Square,
    /// Index into the brush alphas.
    Image(usize),
}

#[derive(Serialize, Deserialize)]
pub(crate) struct FileBrushMaterial {
    channels: Vec<String>,
    values: FileValues,
    grayscale: f32,
}

#[derive(Serialize, Deserialize)]
struct FileCamera {
    /// Column by column.
    view_proj: [f32; 16],
    eye: [f32; 3],
    forward: [f32; 3],
    aspect: f32,
    orthographic: bool,
    height: f32,
}

#[derive(Serialize, Deserialize)]
struct FileImage {
    file: String,
    name: String,
    width: u32,
    height: u32,
    texels: FileTexels,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FileTexels {
    Rgba8,
    Rgba16,
    #[serde(rename = "rgba32f")]
    RgbaF32,
}

#[derive(Serialize, Deserialize)]
struct FileAlpha {
    file: String,
    name: String,
    width: u32,
    height: u32,
}

#[derive(Serialize, Deserialize)]
struct FileBake {
    maps: Vec<String>,
    subsampling: u32,
    dilation: u32,
    front_distance: f32,
    rear_distance: f32,
    average_normals: bool,
    ignore_backfaces: bool,
    matching: String,
    id_source: String,
    ao_rays: u32,
    ao_distance: f32,
    ao_self_occlusion: bool,
    thickness_rays: u32,
    thickness_distance: f32,
    curvature_radius: f32,
    high_poly: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct FileMeshMap {
    map: String,
    /// Index into the top-level `images`.
    image: usize,
}

#[derive(Serialize, Deserialize)]
struct FileExport {
    directory: String,
    template: String,
    format: String,
    bit_depth: String,
    padding: String,
    dilation: u32,
}

// ---- Saving ----

#[derive(Default)]
struct Saver {
    dabs: Vec<u8>,
    dab_count: u64,
    textures: Vec<Arc<TextureImage>>,
    texture_index: HashMap<u64, usize>,
    alphas: Vec<Arc<AlphaImage>>,
    alpha_index: HashMap<u64, usize>,
}

impl Saver {
    fn project(&mut self, project: &Project) -> Result<FileProject> {
        let model = &project.model;
        let texture_sets = model
            .texture_sets
            .iter()
            .map(|set| FileTextureSet {
                name: set.name.clone(),
                visible: set.visible,
                resolution: set.resolution,
                alpha_mode: match set.alpha_mode {
                    AlphaMode::Opaque => FileAlphaMode::Opaque,
                    AlphaMode::Test => FileAlphaMode::Test,
                    AlphaMode::Blend => FileAlphaMode::Blend,
                },
                layers: set.stack.layers.iter().map(|l| self.layer(l)).collect(),
                mesh_maps: set
                    .mesh_maps
                    .maps
                    .iter()
                    .map(|(map, image)| FileMeshMap {
                        map: map.key().to_owned(),
                        image: self.texture(image),
                    })
                    .collect(),
            })
            .collect();
        let images = self
            .textures
            .iter()
            .enumerate()
            .map(|(i, image)| FileImage {
                file: format!("images/{i}.{}", texture_extension(image)),
                name: image.name.clone(),
                width: image.width,
                height: image.height,
                texels: match image.texels {
                    Texels::Rgba8(_) => FileTexels::Rgba8,
                    Texels::Rgba16(_) => FileTexels::Rgba16,
                    Texels::RgbaF32(_) => FileTexels::RgbaF32,
                },
            })
            .collect();
        let alphas = self
            .alphas
            .iter()
            .enumerate()
            .map(|(i, alpha)| FileAlpha {
                file: format!("alphas/{i}.png"),
                name: alpha.name.clone(),
                width: alpha.width,
                height: alpha.height,
            })
            .collect();
        Ok(FileProject {
            format: FORMAT.to_owned(),
            version: VERSION,
            generator: concat!("inkpainter3d ", env!("CARGO_PKG_VERSION")).to_owned(),
            name: model.name.clone(),
            source: project.source.clone(),
            normal_format: match model.normal_format {
                NormalFormat::OpenGl => FileNormalFormat::OpenGl,
                NormalFormat::DirectX => FileNormalFormat::DirectX,
            },
            last_id: model.ids.last(),
            mesh: FileMesh {
                vertices: u32::try_from(model.mesh.vertex_count()).context("Too many vertices")?,
                indices: u32::try_from(model.mesh.indices.len()).context("Too many triangles")?,
            },
            objects: model
                .objects
                .iter()
                .map(|o| FileObject {
                    name: o.name.clone(),
                    visible: o.visible,
                })
                .collect(),
            texture_sets,
            parts: model
                .parts
                .iter()
                .map(|p| FilePart {
                    object: p.object,
                    texture_set: p.texture_set,
                    first_index: p.indices.start,
                    index_count: p.indices.end - p.indices.start,
                })
                .collect(),
            images,
            alphas,
            view: project.view.as_ref().map(finite_view),
            bake: project.bake.as_ref().map(file_bake),
            export: project.export.as_ref().map(|e| FileExport {
                directory: e.directory.clone(),
                template: e.template.key.to_owned(),
                format: e.format.label().to_owned(),
                bit_depth: e.bit_depth.key().to_owned(),
                padding: e.padding.key().to_owned(),
                dilation: e.dilation,
            }),
        })
    }

    fn layer(&mut self, layer: &Layer) -> FileLayer {
        let blending = Channel::ALL
            .iter()
            .map(|&c| {
                let blend = layer.blending.get(c);
                (
                    c.key().to_owned(),
                    FileBlend {
                        mode: blend.mode.key().to_owned(),
                        opacity: finite(blend.opacity),
                    },
                )
            })
            .collect();
        let content = match &layer.content {
            Content::Paint(paint) => FileContent::Paint(self.paint(paint)),
            Content::Fill(fill) => FileContent::Fill(FileFill {
                channels: channel_keys(fill.channels),
                values: values(&fill.values),
                images: Channel::ALL
                    .iter()
                    .filter_map(|&c| Some((c.key().to_owned(), self.texture(fill.image(c)?))))
                    .collect(),
                normal_directx: fill.normal_directx,
                projection: projection(&fill.projection),
            }),
            Content::Folder(folder) => FileContent::Folder(FileFolder {
                expanded: folder.expanded,
                layers: folder.layers.iter().map(|l| self.layer(l)).collect(),
            }),
        };
        FileLayer {
            id: layer.id,
            name: layer.name.clone(),
            visible: layer.visible,
            blending,
            content,
            mask: layer.mask.as_ref().map(|mask| FileMask {
                enabled: mask.enabled,
                background: finite(mask.background),
                paint: self.paint(&mask.paint),
                effects: mask
                    .effects
                    .iter()
                    .map(|effect| FileEffect {
                        id: effect.id,
                        visible: effect.visible,
                        blend: effect.blend.key().to_owned(),
                        opacity: finite(effect.opacity),
                        kind: match &effect.kind {
                            EffectKind::Paint(paint) => FileEffectKind::Paint(self.paint(paint)),
                            EffectKind::Fill(gray) => FileEffectKind::Fill(FileGrayFill {
                                value: finite(gray.value),
                                image: gray.image.as_ref().map(|i| self.texture(i)),
                                projection: projection(&gray.projection),
                            }),
                            EffectKind::Levels(levels) => FileEffectKind::Levels(FileLevels {
                                input: finite_n(levels.input),
                                gamma: finite(levels.gamma),
                                output: finite_n(levels.output),
                            }),
                            EffectKind::Generator(g) => FileEffectKind::Generator(FileGenerator {
                                kind: g.kind.key().to_owned(),
                                params: g
                                    .kind
                                    .params()
                                    .iter()
                                    .zip(g.values)
                                    .map(|(p, v)| (p.key.to_owned(), finite(v)))
                                    .collect(),
                            }),
                        },
                    })
                    .collect(),
            }),
        }
    }

    fn paint(&mut self, paint: &PaintLayer) -> FilePaint {
        FilePaint {
            id: paint.id,
            strokes: paint.strokes.iter().map(|s| self.stroke(s)).collect(),
        }
    }

    fn stroke(&mut self, stroke: &Stroke) -> FileStroke {
        let first_dab = self.dab_count;
        for dab in &stroke.dabs {
            for v in [dab.position, dab.normal, dab.up, dab.view] {
                for c in v.to_array() {
                    self.dabs.extend_from_slice(&c.to_le_bytes());
                }
            }
            self.dabs.extend_from_slice(&dab.radius.to_le_bytes());
            self.dabs.extend_from_slice(&dab.flow.to_le_bytes());
        }
        self.dab_count += stroke.dabs.len() as u64;
        FileStroke {
            id: stroke.id,
            tool: match stroke.tool {
                Tool::Paint => FileTool::Paint,
                Tool::Erase => FileTool::Erase,
            },
            space: match stroke.space {
                StrokeSpace::Surface => FileSpace::Surface,
                StrokeSpace::Uv => FileSpace::Uv,
            },
            alpha: match &stroke.alpha {
                Alpha::Round => FileAlphaShape::Round,
                Alpha::Square => FileAlphaShape::Square,
                Alpha::Image(image) => FileAlphaShape::Image(self.alpha(image)),
            },
            hardness: finite(stroke.hardness),
            alignment: Some(stroke.alignment.key().to_owned()),
            opacity: finite(stroke.opacity),
            backface_cos: finite(stroke.backface_cos),
            material: FileBrushMaterial {
                channels: channel_keys(stroke.material.channels),
                values: values(&stroke.material.values),
                grayscale: finite(stroke.material.grayscale),
            },
            camera: stroke.camera.map(|c| FileCamera {
                view_proj: finite_n(c.view_proj.to_cols_array()),
                eye: finite_n(c.eye.to_array()),
                forward: finite_n(c.forward.to_array()),
                aspect: finite(c.aspect),
                orthographic: c.orthographic,
                height: finite(c.height),
            }),
            hidden_objects: stroke.hidden_objects.clone(),
            hidden_sets: stroke.hidden_sets.clone(),
            fill: stroke.fill.as_ref().map(|f| match f {
                FillTarget::Triangles(list) => FileStrokeFill::Triangles(list.to_vec()),
                FillTarget::Object(o) => FileStrokeFill::Object(*o),
            }),
            stencil: stroke.stencil.as_ref().map(|s| FileStencil {
                alpha: self.alpha(&s.image),
                affine: finite_n(s.affine),
            }),
            first_dab,
            dab_count: stroke.dabs.len() as u64,
        }
    }

    fn texture(&mut self, image: &Arc<TextureImage>) -> usize {
        *self.texture_index.entry(image.id).or_insert_with(|| {
            self.textures.push(image.clone());
            self.textures.len() - 1
        })
    }

    fn alpha(&mut self, image: &Arc<AlphaImage>) -> usize {
        *self.alpha_index.entry(image.id).or_insert_with(|| {
            self.alphas.push(image.clone());
            self.alphas.len() - 1
        })
    }
}

pub(crate) fn material_to_file(m: &BrushMaterial) -> FileBrushMaterial {
    FileBrushMaterial {
        channels: channel_keys(m.channels),
        values: values(&m.values),
        grayscale: finite(m.grayscale),
    }
}

pub(crate) fn material_from_file(m: FileBrushMaterial) -> BrushMaterial {
    BrushMaterial {
        channels: channel_set(&m.channels),
        values: channel_values(m.values),
        grayscale: m.grayscale,
    }
}

pub(crate) fn channel_keys(set: ChannelSet) -> Vec<String> {
    set.iter().map(|c| c.key().to_owned()).collect()
}

/// JSON has no NaN or infinity: serde_json writes them as null, which no reader accepts.
/// Such values are saved as 0 and the largest finite values.
pub(crate) fn finite(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(f32::MIN, f32::MAX)
    }
}

fn finite_n<const N: usize>(v: [f32; N]) -> [f32; N] {
    v.map(finite)
}

pub(crate) fn values(v: &ChannelValues) -> FileValues {
    FileValues {
        base_color: finite_n(v.base_color),
        height: finite(v.height),
        roughness: finite(v.roughness),
        metallic: finite(v.metallic),
        normal: finite_n(v.normal),
        emissive: finite_n(v.emissive),
        opacity: finite(v.opacity),
    }
}

fn finite_view(v: &ViewState) -> ViewState {
    ViewState {
        camera_target: finite_n(v.camera_target),
        camera_yaw: finite(v.camera_yaw),
        camera_pitch: finite(v.camera_pitch),
        camera_distance: finite(v.camera_distance),
        camera_fov_y: finite(v.camera_fov_y),
        environment_rotation: finite(v.environment_rotation),
        exposure: finite(v.exposure),
        background_opacity: finite(v.background_opacity),
        background_blur: finite(v.background_blur),
        outline_color: finite_n(v.outline_color),
        outline_width: finite(v.outline_width),
        ..v.clone()
    }
}

fn projection(p: &Projection) -> FileProjection {
    FileProjection {
        mode: match p.mode {
            ProjectionMode::Uv => FileProjectionMode::Uv,
            ProjectionMode::TriPlanar => FileProjectionMode::TriPlanar,
        },
        scale: finite_n(p.scale),
        rotation: finite(p.rotation),
        offset: finite_n(p.offset),
        hardness: finite(p.hardness),
    }
}

/// Bytes per vertex in `mesh.bin`: position, normal, UV and tangent.
const VERTEX_BYTES: usize = (3 + 3 + 2 + 4) * 4;

/// Little-endian: all positions, then normals, UVs, tangents and indices.
fn mesh_bytes(mesh: &Mesh) -> Vec<u8> {
    let mut out = Vec::with_capacity(mesh.vertex_count() * VERTEX_BYTES + mesh.indices.len() * 4);
    let mut floats = |values: &mut dyn Iterator<Item = f32>| {
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    };
    floats(&mut mesh.positions.iter().flat_map(|v| v.to_array()));
    floats(&mut mesh.normals.iter().flat_map(|v| v.to_array()));
    floats(&mut mesh.uvs.iter().flat_map(|v| v.to_array()));
    floats(&mut mesh.tangents.iter().flat_map(|v| v.to_array()));
    for i in &mesh.indices {
        out.extend_from_slice(&i.to_le_bytes());
    }
    out
}

fn texture_extension(image: &TextureImage) -> &'static str {
    match image.texels {
        Texels::RgbaF32(_) => "exr",
        _ => "png",
    }
}

fn extension_of(file: &str) -> &'static str {
    if file.ends_with(".exr") { "exr" } else { "png" }
}

/// PNG for 8 and 16 bits, OpenEXR for floats: lossless, so the texels come back exactly.
fn encode_texture(image: &TextureImage) -> Result<Encoded> {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    let (w, h) = (image.width, image.height);
    match &image.texels {
        Texels::Rgba8(texels) => {
            png_encoder(&mut bytes).write_image(texels, w, h, image::ExtendedColorType::Rgba8)?
        }
        Texels::Rgba16(texels) => {
            let raw: Vec<u8> = texels.iter().flat_map(|v| v.to_ne_bytes()).collect();
            png_encoder(&mut bytes).write_image(&raw, w, h, image::ExtendedColorType::Rgba16)?
        }
        Texels::RgbaF32(texels) => {
            let raw: Vec<u8> = texels.iter().flat_map(|v| v.to_ne_bytes()).collect();
            image::codecs::openexr::OpenExrEncoder::new(std::io::Cursor::new(&mut bytes))
                .write_image(&raw, w, h, image::ExtendedColorType::Rgba32F)?
        }
    }
    Ok(Encoded {
        bytes: Arc::new(bytes),
    })
}

pub(crate) fn encode_alpha(alpha: &AlphaImage) -> Result<Encoded> {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    png_encoder(&mut bytes).write_image(
        &alpha.pixels,
        alpha.width,
        alpha.height,
        image::ExtendedColorType::L8,
    )?;
    Ok(Encoded {
        bytes: Arc::new(bytes),
    })
}

pub(crate) fn png_encoder(bytes: &mut Vec<u8>) -> image::codecs::png::PngEncoder<&mut Vec<u8>> {
    image::codecs::png::PngEncoder::new_with_quality(
        bytes,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
}

// ---- Loading ----

fn read_mesh(data: &[u8], vertices: usize, indices: usize) -> Result<Mesh> {
    ensure!(
        data.len() == vertices * VERTEX_BYTES + indices * 4,
        "The mesh is damaged"
    );
    ensure!(indices.is_multiple_of(3), "The mesh is damaged");
    let mut floats = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b));
    let mut take = |n: usize| -> Vec<f32> { floats.by_ref().take(n).collect() };
    let positions = take(vertices * 3);
    let normals = take(vertices * 3);
    let uvs = take(vertices * 2);
    let tangents = take(vertices * 4);
    let index_data = &data[vertices * VERTEX_BYTES..];
    let indices: Vec<u32> = index_data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect();
    ensure!(
        indices.iter().all(|&i| (i as usize) < vertices),
        "The mesh is damaged"
    );
    Ok(Mesh {
        name: String::new(),
        positions: positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|v| Vec3::from_array(*v))
            .collect(),
        normals: normals
            .as_chunks::<3>()
            .0
            .iter()
            .map(|v| Vec3::from_array(*v))
            .collect(),
        uvs: uvs
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| Vec2::from_array(*v))
            .collect(),
        tangents: tangents
            .as_chunks::<4>()
            .0
            .iter()
            .map(|v| Vec4::from_array(*v))
            .collect(),
        indices,
    })
}

fn check_size(width: u32, height: u32, max: u32) -> Result<()> {
    ensure!(
        width > 0 && height > 0 && width <= max && height <= max,
        "Unsupported image size {width} × {height}"
    );
    Ok(())
}

/// Decodes an image whose size is already checked, without the image crate's default
/// memory limit, which large HDR fills exceed.
pub(crate) fn decode(bytes: &[u8], format: image::ImageFormat) -> Result<image::DynamicImage> {
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    reader.no_limits();
    Ok(reader.decode()?)
}

fn decode_texture(entry: &FileImage, bytes: &[u8]) -> Result<TextureImage> {
    let format = match extension_of(&entry.file) {
        "exr" => image::ImageFormat::OpenExr,
        _ => image::ImageFormat::Png,
    };
    let image = decode(bytes, format)?;
    ensure!(
        image.width() == entry.width && image.height() == entry.height,
        "The image has the wrong size"
    );
    let texels = match entry.texels {
        FileTexels::Rgba8 => Texels::Rgba8(image.into_rgba8().into_raw()),
        FileTexels::Rgba16 => Texels::Rgba16(image.into_rgba16().into_raw()),
        FileTexels::RgbaF32 => Texels::RgbaF32(image.into_rgba32f().into_raw()),
    };
    Ok(TextureImage::new(
        entry.name.clone(),
        entry.width,
        entry.height,
        texels,
    ))
}

fn decode_alpha(entry: &FileAlpha, bytes: &[u8]) -> Result<AlphaImage> {
    let image = decode(bytes, image::ImageFormat::Png)?;
    ensure!(
        image.width() == entry.width && image.height() == entry.height,
        "The image has the wrong size"
    );
    Ok(AlphaImage::new(
        entry.name.clone(),
        entry.width,
        entry.height,
        image.into_luma8().into_raw(),
    ))
}

/// The number of dabs of all strokes in the layers; None if it overflows.
fn dab_count(layers: &[FileLayer]) -> Option<u64> {
    let paint = |p: &FilePaint| {
        p.strokes
            .iter()
            .try_fold(0u64, |n, s| n.checked_add(s.dab_count))
    };
    layers.iter().try_fold(0u64, |mut n, layer| {
        match &layer.content {
            FileContent::Paint(p) => n = n.checked_add(paint(p)?)?,
            FileContent::Fill(_) => {}
            FileContent::Folder(f) => n = n.checked_add(dab_count(&f.layers)?)?,
        }
        if let Some(mask) = &layer.mask {
            n = n.checked_add(paint(&mask.paint)?)?;
            for effect in &mask.effects {
                if let FileEffectKind::Paint(p) = &effect.kind {
                    n = n.checked_add(paint(p)?)?;
                }
            }
        }
        Some(n)
    })
}

/// The most an image file of this size can take: its texels uncompressed at 16 bytes,
/// and room for headers.
fn encoded_limit(width: u32, height: u32) -> u64 {
    width as u64 * height as u64 * 16 + (1 << 20)
}

/// The highest id of layers, paint layers, effects and strokes.
fn highest_id(layers: &[FileLayer]) -> u64 {
    let paint = |p: &FilePaint| p.strokes.iter().map(|s| s.id).fold(p.id, u64::max);
    layers
        .iter()
        .map(|layer| {
            let mut highest = layer.id;
            match &layer.content {
                FileContent::Paint(p) => highest = highest.max(paint(p)),
                FileContent::Fill(_) => {}
                FileContent::Folder(f) => highest = highest.max(highest_id(&f.layers)),
            }
            if let Some(mask) = &layer.mask {
                highest = highest.max(paint(&mask.paint));
                for effect in &mask.effects {
                    highest = highest.max(effect.id);
                    if let FileEffectKind::Paint(p) = &effect.kind {
                        highest = highest.max(paint(p));
                    }
                }
            }
            highest
        })
        .max()
        .unwrap_or(0)
}

struct Loader<'a> {
    dabs: &'a [u8],
    /// Where the next stroke's dabs start: strokes lie one after the other.
    next_dab: u64,
    textures: &'a [Arc<TextureImage>],
    alphas: &'a [Arc<AlphaImage>],
    /// Ids of layers, paint layers and effects seen so far.
    ids: HashSet<u64>,
    /// For ids that are taken twice: the renderer and undo tell things apart by them.
    next_id: u64,
}

impl Loader<'_> {
    fn project(mut self, file: FileProject, mut mesh: Mesh) -> Result<Project> {
        let objects: Vec<MeshObject> = file
            .objects
            .into_iter()
            .map(|o| MeshObject {
                name: o.name,
                visible: o.visible,
            })
            .collect();
        let mut texture_sets = Vec::with_capacity(file.texture_sets.len());
        for set in file.texture_sets {
            ensure!(
                set.resolution.is_power_of_two() && (16..=MAX_RESOLUTION).contains(&set.resolution),
                "Unsupported texture set size {}",
                set.resolution
            );
            let layers = self.layers(set.layers)?;
            let mut mesh_maps = MeshMaps::default();
            for entry in set.mesh_maps {
                if let Some(map) = MeshMap::from_key(&entry.map) {
                    mesh_maps.insert(map, self.texture(entry.image)?);
                }
            }
            texture_sets.push(TextureSet {
                mesh_maps,
                name: set.name,
                visible: set.visible,
                resolution: set.resolution,
                alpha_mode: match set.alpha_mode {
                    FileAlphaMode::Opaque => AlphaMode::Opaque,
                    FileAlphaMode::Test => AlphaMode::Test,
                    FileAlphaMode::Blend => AlphaMode::Blend,
                },
                stack: LayerStack { layers },
            });
        }

        // Parts cover the index list in order, grouped by texture set.
        let mut parts = Vec::with_capacity(file.parts.len());
        let mut end = 0u32;
        let mut last_set = 0;
        for part in file.parts {
            let end_index = part.first_index.checked_add(part.index_count);
            ensure!(end_index.is_some(), "The mesh parts are damaged");
            ensure!(
                part.object < objects.len()
                    && part.texture_set < texture_sets.len()
                    && part.first_index == end
                    && part.index_count.is_multiple_of(3)
                    && part.texture_set >= last_set,
                "The mesh parts are damaged"
            );
            let indices = part.first_index..part.first_index + part.index_count;
            ensure!(
                indices.end as usize <= mesh.indices.len(),
                "The mesh parts are damaged"
            );
            end = indices.end;
            last_set = part.texture_set;
            parts.push(Part {
                object: part.object,
                texture_set: part.texture_set,
                bounds: bounds(&mesh, &indices),
                indices,
            });
        }
        ensure!(
            end as usize == mesh.indices.len(),
            "The mesh parts are damaged"
        );
        ensure!(!parts.is_empty(), "The project has no mesh");

        mesh.name = file.name.clone();
        let mut model = Model::from_parts(
            file.name,
            mesh,
            objects,
            texture_sets,
            parts,
            Ids::after(self.next_id - 1),
        );
        model.normal_format = match file.normal_format {
            FileNormalFormat::OpenGl => NormalFormat::OpenGl,
            FileNormalFormat::DirectX => NormalFormat::DirectX,
        };
        let export = file.export.map(|e| {
            let format = FileFormat::from_key(&e.format).unwrap_or_default();
            let bit_depth = BitDepth::from_key(&e.bit_depth)
                .filter(|d| format.bit_depths().contains(d))
                .unwrap_or(format.bit_depths()[0]);
            ExportSettings {
                directory: e.directory,
                template: Template::from_key(&e.template)
                    .unwrap_or(&ink_core::export::TEMPLATES[0]),
                format,
                bit_depth,
                padding: Padding::from_key(&e.padding).unwrap_or_default(),
                dilation: e.dilation.min(ExportSettings::MAX_DILATION),
            }
        });
        Ok(Project {
            model,
            source: file.source,
            view: file.view,
            export,
            bake: file.bake.map(bake_from_file),
        })
    }

    /// The id, or a new one if it is taken already.
    fn id(&mut self, id: u64) -> u64 {
        if id != 0 && self.ids.insert(id) {
            return id;
        }
        let new = self.next_id;
        self.next_id += 1;
        self.ids.insert(new);
        new
    }

    fn layers(&mut self, layers: Vec<FileLayer>) -> Result<Vec<Layer>> {
        layers.into_iter().map(|l| self.layer(l)).collect()
    }

    fn layer(&mut self, layer: FileLayer) -> Result<Layer> {
        let id = self.id(layer.id);
        let mut blending = Blending::new_layer();
        for (key, blend) in layer.blending {
            // Channels of newer versions are skipped.
            if let Some(channel) = Channel::from_key(&key) {
                *blending.get_mut(channel) = ChannelBlend {
                    mode: blend_mode(&blend.mode)?,
                    opacity: blend.opacity,
                };
            }
        }
        let content = match layer.content {
            FileContent::Paint(paint) => Content::Paint(self.paint(paint)?),
            FileContent::Fill(fill) => {
                let mut images: [Option<Arc<TextureImage>>; Channel::COUNT] = Default::default();
                for (key, index) in fill.images {
                    if let Some(channel) = Channel::from_key(&key) {
                        images[channel.index()] = Some(self.texture(index)?);
                    }
                }
                Content::Fill(Fill {
                    channels: channel_set(&fill.channels),
                    values: channel_values(fill.values),
                    images,
                    normal_directx: fill.normal_directx,
                    projection: projection_from(fill.projection),
                })
            }
            FileContent::Folder(folder) => Content::Folder(Folder {
                layers: self.layers(folder.layers)?,
                expanded: folder.expanded,
            }),
        };
        let mask = match layer.mask {
            Some(mask) => {
                let paint = self.paint(mask.paint)?;
                let mut effects = Vec::with_capacity(mask.effects.len());
                for effect in mask.effects {
                    let id = self.id(effect.id);
                    let kind = match effect.kind {
                        FileEffectKind::Paint(paint) => EffectKind::Paint(self.paint(paint)?),
                        FileEffectKind::Fill(gray) => EffectKind::Fill(GrayFill {
                            value: gray.value,
                            image: gray.image.map(|i| self.texture(i)).transpose()?,
                            projection: projection_from(gray.projection),
                        }),
                        FileEffectKind::Levels(levels) => EffectKind::Levels(Levels {
                            input: levels.input,
                            gamma: levels.gamma,
                            output: levels.output,
                        }),
                        FileEffectKind::Generator(g) => {
                            let kind = GeneratorKind::from_key(&g.kind)
                                .with_context(|| format!("Unknown generator {:?}", g.kind))?;
                            let mut generator = Generator::new(kind);
                            for (key, value) in g.params {
                                generator.set(&key, value);
                            }
                            EffectKind::Generator(generator)
                        }
                    };
                    effects.push(Effect {
                        id,
                        visible: effect.visible,
                        blend: blend_mode(&effect.blend)?,
                        opacity: effect.opacity,
                        kind,
                    });
                }
                Some(Mask {
                    enabled: mask.enabled,
                    background: mask.background,
                    paint,
                    effects,
                })
            }
            None => None,
        };
        Ok(Layer {
            id,
            name: layer.name,
            visible: layer.visible,
            blending,
            content,
            mask,
        })
    }

    fn paint(&mut self, paint: FilePaint) -> Result<PaintLayer> {
        let id = self.id(paint.id);
        let mut strokes = Vec::with_capacity(paint.strokes.len());
        let mut stroke_ids = HashSet::new();
        for stroke in paint.strokes {
            let mut stroke = self.stroke(stroke)?;
            // Within a paint layer, strokes are told apart by their ids.
            if !stroke_ids.insert(stroke.id) {
                stroke.id = self.next_id;
                self.next_id += 1;
                stroke_ids.insert(stroke.id);
            }
            strokes.push(Arc::new(stroke));
        }
        Ok(PaintLayer { id, strokes })
    }

    fn stroke(&mut self, stroke: FileStroke) -> Result<Stroke> {
        ensure!(
            stroke.first_dab == self.next_dab,
            "The project's strokes are damaged"
        );
        self.next_dab = self.next_dab.saturating_add(stroke.dab_count);
        let dab_bytes = DAB_FLOATS as u64 * 4;
        let start = stroke.first_dab.checked_mul(dab_bytes);
        let end = stroke
            .first_dab
            .checked_add(stroke.dab_count)
            .and_then(|e| e.checked_mul(dab_bytes));
        let (Some(start), Some(end)) = (start, end) else {
            bail!("The project's strokes are damaged");
        };
        ensure!(
            end <= self.dabs.len() as u64,
            "The project's strokes are damaged"
        );
        let dabs = self.dabs[start as usize..end as usize]
            .as_chunks::<{ DAB_FLOATS * 4 }>()
            .0
            .iter()
            .map(|bytes| {
                let floats = bytes.as_chunks::<4>().0;
                let f = |i: usize| f32::from_le_bytes(floats[i]);
                let v3 = |i: usize| Vec3::new(f(i), f(i + 1), f(i + 2));
                Dab {
                    position: v3(0),
                    normal: v3(3),
                    up: v3(6),
                    view: v3(9),
                    radius: f(12),
                    flow: f(13),
                }
            })
            .collect();
        Ok(Stroke {
            id: stroke.id,
            tool: match stroke.tool {
                FileTool::Paint => Tool::Paint,
                FileTool::Erase => Tool::Erase,
            },
            space: match stroke.space {
                FileSpace::Surface => StrokeSpace::Surface,
                FileSpace::Uv => StrokeSpace::Uv,
            },
            alpha: match stroke.alpha {
                FileAlphaShape::Round => Alpha::Round,
                FileAlphaShape::Square => Alpha::Square,
                FileAlphaShape::Image(i) => Alpha::Image(
                    self.alphas
                        .get(i)
                        .cloned()
                        .context("A stroke's brush alpha is missing")?,
                ),
            },
            hardness: stroke.hardness,
            alignment: stroke
                .alignment
                .as_deref()
                .and_then(Alignment::from_key)
                .unwrap_or_default(),
            opacity: stroke.opacity,
            backface_cos: stroke.backface_cos,
            material: BrushMaterial {
                channels: channel_set(&stroke.material.channels),
                values: channel_values(stroke.material.values),
                grayscale: stroke.material.grayscale,
            },
            camera: stroke.camera.map(|c| StrokeCamera {
                view_proj: Mat4::from_cols_array(&c.view_proj),
                eye: Vec3::from_array(c.eye),
                forward: Vec3::from_array(c.forward),
                aspect: c.aspect,
                orthographic: c.orthographic,
                height: c.height,
            }),
            hidden_objects: stroke.hidden_objects,
            hidden_sets: stroke.hidden_sets,
            fill: stroke.fill.map(|f| match f {
                FileStrokeFill::Triangles(list) => FillTarget::Triangles(Arc::new(list)),
                FileStrokeFill::Object(o) => FillTarget::Object(o),
            }),
            stencil: stroke
                .stencil
                .map(|s| -> Result<StrokeStencil> {
                    Ok(StrokeStencil {
                        image: self
                            .alphas
                            .get(s.alpha)
                            .cloned()
                            .context("A stroke's stencil image is missing")?,
                        affine: s.affine,
                    })
                })
                .transpose()?,
            dabs,
        })
    }

    fn texture(&self, index: usize) -> Result<Arc<TextureImage>> {
        self.textures
            .get(index)
            .cloned()
            .context("A fill's image is missing")
    }
}

fn blend_mode(key: &str) -> Result<BlendMode> {
    BlendMode::from_key(key).with_context(|| format!("Unknown blend mode {key}"))
}

pub(crate) fn channel_set(keys: &[String]) -> ChannelSet {
    let mut set = ChannelSet::NONE;
    for channel in keys.iter().filter_map(|k| Channel::from_key(k)) {
        set.set(channel, true);
    }
    set
}

pub(crate) fn channel_values(v: FileValues) -> ChannelValues {
    ChannelValues {
        base_color: v.base_color,
        height: v.height,
        roughness: v.roughness,
        metallic: v.metallic,
        normal: v.normal,
        emissive: v.emissive,
        opacity: v.opacity,
    }
}

fn file_bake(b: &BakeSettings) -> FileBake {
    FileBake {
        maps: b.maps.iter().map(|m| m.key().to_owned()).collect(),
        subsampling: b.subsampling,
        dilation: b.dilation,
        front_distance: finite(b.front_distance),
        rear_distance: finite(b.rear_distance),
        average_normals: b.average_normals,
        ignore_backfaces: b.ignore_backfaces,
        matching: b.matching.key().to_owned(),
        id_source: b.id_source.key().to_owned(),
        ao_rays: b.ao_rays,
        ao_distance: finite(b.ao_distance),
        ao_self_occlusion: b.ao_self_occlusion,
        thickness_rays: b.thickness_rays,
        thickness_distance: finite(b.thickness_distance),
        curvature_radius: finite(b.curvature_radius),
        high_poly: b
            .high_poly
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
    }
}

fn bake_from_file(b: FileBake) -> BakeSettings {
    let defaults = BakeSettings::default();
    BakeSettings {
        maps: b.maps.iter().filter_map(|k| MeshMap::from_key(k)).collect(),
        subsampling: b.subsampling.clamp(1, 4),
        dilation: b.dilation.min(256),
        front_distance: b.front_distance.clamp(0.0, 1.0),
        rear_distance: b.rear_distance.clamp(0.0, 1.0),
        average_normals: b.average_normals,
        ignore_backfaces: b.ignore_backfaces,
        matching: Matching::from_key(&b.matching).unwrap_or(defaults.matching),
        id_source: IdSource::from_key(&b.id_source).unwrap_or(defaults.id_source),
        ao_rays: b.ao_rays.clamp(1, 4096),
        ao_distance: b.ao_distance.clamp(0.0, 10.0),
        ao_self_occlusion: b.ao_self_occlusion,
        thickness_rays: b.thickness_rays.clamp(1, 4096),
        thickness_distance: b.thickness_distance.clamp(0.0, 10.0),
        curvature_radius: b.curvature_radius.clamp(0.0, 1.0),
        high_poly: b.high_poly.into_iter().map(PathBuf::from).collect(),
    }
}

fn projection_from(p: FileProjection) -> Projection {
    Projection {
        mode: match p.mode {
            FileProjectionMode::Uv => ProjectionMode::Uv,
            FileProjectionMode::TriPlanar => ProjectionMode::TriPlanar,
        },
        scale: p.scale,
        rotation: p.rotation,
        offset: p.offset,
        hardness: p.hardness,
    }
}

fn bounds(mesh: &Mesh, indices: &std::ops::Range<u32>) -> Aabb {
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for &i in &mesh.indices[indices.start as usize..indices.end as usize] {
        let p = mesh.positions[i as usize];
        min = min.min(p);
        max = max.max(p);
    }
    Aabb { min, max }
}

/// The path a project made from `mesh` is saved to by default: next to the mesh, with the
/// project extension.
pub fn default_path(mesh: &Path) -> PathBuf {
    mesh.with_extension(PROJECT_EXTENSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ink_core::{LayerKind, ModelBuilder};
    use std::io::Cursor;

    /// Two objects in two texture sets, with every kind of layer, mask and effect, strokes
    /// with and without camera and brush alpha, and images of every texel type.
    fn project() -> Project {
        let mut builder = ModelBuilder::default();
        let cube = Mesh::cube(1.0);
        for (name, material, offset) in [("Body", "Skin", 0.0), ("Belt", "Leather", 3.0)] {
            let object = builder.add_object(name);
            let set = builder.texture_set(material);
            let base = builder.mesh.positions.len() as u32;
            builder
                .mesh
                .positions
                .extend(cube.positions.iter().map(|p| *p + Vec3::X * offset));
            builder.mesh.normals.extend(&cube.normals);
            builder.mesh.uvs.extend(&cube.uvs);
            for tri in cube.indices.as_chunks::<3>().0 {
                builder.triangle(object, set, tri.map(|i| i + base));
            }
        }
        let mut model = builder.build("Cat".into());
        model.normal_format = NormalFormat::OpenGl;
        model.objects[1].visible = false;
        model.texture_sets[1].resolution = 512;
        model.texture_sets[1].alpha_mode = AlphaMode::Test;

        let rgba8 = Arc::new(TextureImage::new(
            "rust".into(),
            2,
            1,
            Texels::Rgba8(vec![1, 2, 3, 4, 250, 251, 252, 253]),
        ));
        let rgba16 = Arc::new(TextureImage::new(
            "height".into(),
            1,
            2,
            Texels::Rgba16(vec![0, 1, 65534, 65535, 300, 40000, 7, 9]),
        ));
        let float = Arc::new(TextureImage::new(
            "sky".into(),
            1,
            1,
            Texels::RgbaF32(vec![0.1, 7.5, -0.25, 1.0]),
        ));
        let alpha = Arc::new(AlphaImage::new("dots".into(), 2, 2, vec![0, 64, 128, 255]));

        let stroke = |model: &mut Model, alpha: Alpha, camera: bool| {
            let stencil = match &alpha {
                Alpha::Image(image) if camera => Some(StrokeStencil {
                    image: image.clone(),
                    affine: [1.0, 0.0, 0.5, 0.0, -2.0, 0.25],
                }),
                _ => None,
            };
            let dab = |x: f32| Dab {
                position: Vec3::new(x, 0.25, -1.5),
                normal: Vec3::Z,
                up: Vec3::Y,
                view: Vec3::NEG_Z,
                radius: 0.1 + x,
                flow: 0.75,
            };
            Arc::new(Stroke {
                id: model.next_id(),
                tool: Tool::Paint,
                space: StrokeSpace::Surface,
                alpha,
                hardness: 0.3,
                alignment: Alignment::Camera,
                opacity: 0.9,
                backface_cos: 0.1,
                material: BrushMaterial::default(),
                camera: camera.then(|| StrokeCamera {
                    view_proj: Mat4::from_cols_array(&std::array::from_fn(|i| i as f32 * 0.5)),
                    eye: Vec3::new(1.0, 2.0, 3.0),
                    forward: Vec3::NEG_Z,
                    aspect: 1.6,
                    orthographic: true,
                    height: 0.8,
                }),
                hidden_objects: vec![1],
                hidden_sets: vec![],
                fill: camera.then(|| FillTarget::Triangles(Arc::new(vec![0, 2, 1]))),
                stencil,
                dabs: vec![dab(0.0), dab(0.125), dab(1e-7)],
            })
        };

        let s1 = stroke(&mut model, Alpha::Image(alpha.clone()), true);
        let s2 = stroke(&mut model, Alpha::Square, false);
        let s3 = stroke(&mut model, Alpha::Image(alpha), false);
        let ids = &mut model.ids;
        model.texture_sets[1].mesh_maps.insert(
            MeshMap::Curvature,
            Arc::new(TextureImage::new(
                "Curvature".into(),
                2,
                2,
                Texels::Rgba16((0..16).map(|i| i * 4000).collect()),
            )),
        );
        let stack = &mut model.texture_sets[0].stack;
        if let Content::Paint(paint) = &mut stack.layers[0].content {
            paint.strokes = vec![s1, s2];
        }
        let mut fill = Layer::new(LayerKind::Fill, "Rust".into(), ids);
        if let Content::Fill(f) = &mut fill.content {
            f.channels = ChannelSet::ALL;
            f.values.base_color = [0.1, 0.2, 0.3];
            f.images[Channel::BaseColor.index()] = Some(rgba8);
            f.images[Channel::Height.index()] = Some(rgba16.clone());
            f.images[Channel::Emissive.index()] = Some(float);
            f.normal_directx = false;
            f.projection = Projection {
                mode: ProjectionMode::TriPlanar,
                scale: [2.0, 3.0],
                rotation: 30.0,
                offset: [0.5, -0.25],
                hardness: 0.75,
            };
        }
        fill.blending.get_mut(Channel::Roughness).mode = BlendMode::Multiply;
        fill.blending.get_mut(Channel::Roughness).opacity = 0.4;
        let mut mask = Mask::new(0.0, ids.next());
        mask.paint.strokes = vec![s3];
        let mut effect = Effect::new(ids.next(), EffectKind::Paint(PaintLayer::new(ids.next())));
        effect.blend = BlendMode::Screen;
        effect.opacity = 0.5;
        mask.effects.push(effect);
        mask.effects.push(Effect::new(
            ids.next(),
            EffectKind::Fill(GrayFill {
                value: 0.3,
                image: Some(rgba16),
                projection: Projection::default(),
            }),
        ));
        let mut levels = Effect::new(
            ids.next(),
            EffectKind::Levels(Levels {
                input: [0.1, 0.9],
                gamma: 1.5,
                output: [1.0, 0.0],
            }),
        );
        levels.visible = false;
        mask.effects.push(levels);
        let mut wear = Generator::new(GeneratorKind::EdgeWear);
        wear.set("noise_scale", 33.0);
        wear.set("invert", 1.0);
        mask.effects
            .push(Effect::new(ids.next(), EffectKind::Generator(wear)));
        mask.enabled = false;
        fill.mask = Some(mask);
        let mut folder = Layer::new(LayerKind::Folder, "Dirt".into(), ids);
        folder.blending = Blending::uniform(BlendMode::Passthrough);
        if let Content::Folder(f) = &mut folder.content {
            f.expanded = false;
            f.layers.push(fill);
            f.layers
                .push(Layer::new(LayerKind::Paint, "Scratches".into(), ids));
        }
        folder.visible = false;
        stack.layers.push(folder);

        Project {
            model,
            source: Some("/models/cat.fbx".into()),
            view: Some(ViewState {
                camera_target: [0.0, 1.0, 0.0],
                camera_yaw: 0.5,
                camera_pitch: 0.25,
                camera_distance: 3.0,
                camera_fov_y: 0.7,
                orthographic: false,
                stylized: true,
                environment: None,
                environment_rotation: 1.0,
                exposure: -0.5,
                background_opacity: 0.2,
                background_blur: 0.4,
                toon_bands: 4,
                outline: false,
                outline_color: [0.1, 0.2, 0.3],
                outline_width: 1.5,
            }),
            bake: Some(BakeSettings {
                maps: vec![MeshMap::Curvature, MeshMap::Height],
                subsampling: 2,
                high_poly: vec![PathBuf::from("/models/cat_high.fbx")],
                ..BakeSettings::default()
            }),
            export: Some(ExportSettings {
                directory: "/tmp/out".into(),
                template: Template::from_key("godot_orm").unwrap(),
                format: FileFormat::Png,
                bit_depth: BitDepth::Sixteen,
                padding: Padding::Background,
                dilation: 8,
            }),
        }
    }

    fn to_bytes(project: &Project) -> Vec<u8> {
        let mut bytes = Vec::new();
        write(Cursor::new(&mut bytes), project, &mut ImageCache::default()).unwrap();
        bytes
    }

    fn entry(bytes: &[u8], name: &str) -> Vec<u8> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        read_entry(&mut zip, name, u64::MAX >> 1).unwrap()
    }

    #[test]
    fn projects_come_back_unchanged() {
        let original = project();
        let saved = to_bytes(&original);
        let (loaded, _) = read(Cursor::new(&saved)).unwrap();
        let again = to_bytes(&loaded);
        // Everything in the files is the same: structure, mesh, strokes and images.
        for name in [
            PROJECT_JSON,
            MESH_BIN,
            DABS_BIN,
            "images/0.png",
            "images/1.png",
        ] {
            assert!(entry(&saved, name) == entry(&again, name), "{name} differs");
        }
        assert_eq!(entry(&saved, "images/2.exr"), entry(&again, "images/2.exr"));
        assert_eq!(entry(&saved, "alphas/0.png"), entry(&again, "alphas/0.png"));

        let (a, b) = (&original.model, &loaded.model);
        assert_eq!(a.mesh.positions, b.mesh.positions);
        assert_eq!(a.mesh.tangents, b.mesh.tangents);
        assert_eq!(a.mesh.indices, b.mesh.indices);
        assert_eq!(a.parts, b.parts);
        assert_eq!(b.normal_format, NormalFormat::OpenGl);
        assert_eq!(b.ids.last(), a.ids.last());
        assert_eq!(loaded.view, original.view);
        assert_eq!(loaded.export, original.export);
        assert_eq!(loaded.bake, original.bake);
        let curvature = |m: &Model| match &m.texture_sets[1]
            .mesh_maps
            .get(MeshMap::Curvature)
            .unwrap()
            .texels
        {
            Texels::Rgba16(v) => v.clone(),
            _ => panic!("stored as 16 bit"),
        };
        assert_eq!(curvature(&loaded.model), curvature(&original.model));
        assert_eq!(loaded.source, original.source);
        assert!(!b.objects[1].visible);
        assert_eq!(b.texture_sets[1].resolution, 512);
        assert_eq!(b.texture_sets[1].alpha_mode, AlphaMode::Test);

        // Strokes, images and alphas are exact; images shared before are shared after.
        let paint = |m: &Model| {
            m.texture_sets[0]
                .stack
                .paint_layers()
                .cloned()
                .collect::<Vec<_>>()
        };
        for (pa, pb) in paint(a).iter().zip(paint(b)) {
            assert_eq!(pa.strokes.len(), pb.strokes.len());
            for (sa, sb) in pa.strokes.iter().zip(&pb.strokes) {
                assert_eq!(sa.dabs, sb.dabs);
                assert_eq!(sa.fill, sb.fill);
                assert_eq!(
                    sa.stencil.as_ref().map(|s| (&s.image.pixels, s.affine)),
                    sb.stencil.as_ref().map(|s| (&s.image.pixels, s.affine))
                );
                assert_eq!(sa.camera, sb.camera);
                assert!(sa.alpha.same(&sa.alpha));
                if let (Alpha::Image(ia), Alpha::Image(ib)) = (&sa.alpha, &sb.alpha) {
                    assert_eq!(ia.pixels, ib.pixels);
                }
            }
        }
        let fill = b.texture_sets[0]
            .stack
            .iter()
            .find_map(|l| match &l.content {
                Content::Fill(f) => Some((f, l.mask.as_ref().unwrap())),
                _ => None,
            })
            .unwrap();
        let height = fill.0.image(Channel::Height).unwrap();
        let EffectKind::Fill(gray) = &fill.1.effects[1].kind else {
            panic!()
        };
        assert!(Arc::ptr_eq(height, gray.image.as_ref().unwrap()));
        assert!(matches!(&height.texels, Texels::Rgba16(t) if t[..4] == [0, 1, 65534, 65535]));
        let emissive = fill.0.image(Channel::Emissive).unwrap();
        assert!(matches!(&emissive.texels, Texels::RgbaF32(t) if t[..] == [0.1, 7.5, -0.25, 1.0]));
    }

    #[test]
    fn saving_again_reuses_encoded_images() {
        let project = project();
        let mut cache = ImageCache::default();
        write(Cursor::new(Vec::new()), &project, &mut cache).unwrap();
        assert_eq!(cache.textures.len(), 4);
        assert_eq!(cache.alphas.len(), 1);
        let before = cache.textures.values().next().unwrap().bytes.clone();
        write(Cursor::new(Vec::new()), &project, &mut cache).unwrap();
        assert!(
            cache
                .textures
                .values()
                .any(|e| Arc::ptr_eq(&e.bytes, &before))
        );
        // Images the project no longer uses are forgotten.
        let mut bare = project.clone();
        bare.model.texture_sets[0].stack.layers.truncate(1);
        write(Cursor::new(Vec::new()), &bare, &mut cache).unwrap();
        // Only the baked map is left.
        assert_eq!(cache.textures.len(), 1);
        bare.model.texture_sets[1].mesh_maps = Default::default();
        write(Cursor::new(Vec::new()), &bare, &mut cache).unwrap();
        assert!(cache.textures.is_empty());
    }

    #[test]
    fn non_finite_values_still_open() {
        let mut project = project();
        let layer = &mut project.model.texture_sets[0].stack.layers[0];
        layer.blending.get_mut(Channel::Roughness).opacity = f32::NAN;
        if let Some(view) = &mut project.view {
            view.exposure = f32::INFINITY;
        }
        let (loaded, _) = read(Cursor::new(to_bytes(&project))).unwrap();
        let layer = &loaded.model.texture_sets[0].stack.layers[0];
        assert_eq!(layer.blending.get(Channel::Roughness).opacity, 0.0);
        assert_eq!(loaded.view.unwrap().exposure, f32::MAX);
    }

    #[test]
    fn duplicate_ids_get_new_ones() {
        let mut project = project();
        let mut copy = project.model.texture_sets[0].stack.layers[0].clone();
        copy.name = "Copy".into();
        project.model.texture_sets[1].stack.layers.push(copy);
        let (loaded, _) = read(Cursor::new(to_bytes(&project))).unwrap();
        let model = &loaded.model;
        let mut ids: Vec<u64> = model
            .texture_sets
            .iter()
            .flat_map(|s| {
                s.stack
                    .iter()
                    .map(|l| l.id)
                    .chain(s.stack.paint_layers().map(|p| p.id))
                    .collect::<Vec<_>>()
            })
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "ids are unique again");
        assert!(model.ids.last() >= *ids.last().unwrap());
    }

    #[test]
    fn refuses_newer_and_damaged_files() {
        let bytes = to_bytes(&project());
        let rewrite = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut json: serde_json::Value =
                serde_json::from_slice(&entry(&bytes, PROJECT_JSON)).unwrap();
            edit(&mut json);
            let mut zip_in = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
            let mut out = Vec::new();
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut out));
            for i in 0..zip_in.len() {
                let mut file = zip_in.by_index(i).unwrap();
                let name = file.name().unwrap().into_owned();
                zip.start_file(&name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                if name == PROJECT_JSON {
                    zip.write_all(&serde_json::to_vec(&json).unwrap()).unwrap();
                } else {
                    std::io::copy(&mut file, &mut zip).unwrap();
                }
            }
            zip.finish().unwrap();
            read(Cursor::new(out)).map(|_| ())
        };
        assert!(rewrite(&|_| {}).is_ok());
        let newer = rewrite(&|j| j["version"] = 99.into()).unwrap_err();
        assert!(format!("{newer:#}").contains("newer version"), "{newer:#}");
        assert!(rewrite(&|j| j["parts"][0]["index_count"] = 3.into()).is_err());
        assert!(rewrite(&|j| j["mesh"]["indices"] = 6.into()).is_err());
        assert!(rewrite(&|j| j["texture_sets"][0]["resolution"] = 1000.into()).is_err());
        let stroke = "/texture_sets/0/layers/0/content/paint/strokes/0/dab_count";
        assert!(rewrite(&|j| *j.pointer_mut(stroke).unwrap() = 1_000_000.into()).is_err());
        // Strokes may not share dabs.
        let second = "/texture_sets/0/layers/0/content/paint/strokes/1/first_dab";
        assert!(rewrite(&|j| *j.pointer_mut(second).unwrap() = 0.into()).is_err());
        assert!(rewrite(&|j| j["parts"][1]["index_count"] = u32::MAX.into()).is_err());
        assert!(rewrite(&|j| j["last_id"] = u64::MAX.into()).is_err());
        assert!(rewrite(&|j| j["texture_sets"][0]["resolution"] = 16384.into()).is_err());
        assert!(rewrite(&|j| j["images"][0]["width"] = 100_000.into()).is_err());
        // Settings that are not understood are dropped, the project still opens.
        assert!(rewrite(&|j| j["view"] = "nonsense".into()).is_ok());
        assert!(read(Cursor::new(b"not a zip".to_vec())).is_err());
    }

    #[test]
    fn saves_replace_files_only_when_complete() {
        let dir = std::env::temp_dir().join(format!("ink_project_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cat.ink3d");
        let project = project();
        save(&path, &project, &mut ImageCache::default()).unwrap();
        let (loaded, _) = load(&path).unwrap();
        assert_eq!(loaded.model.texture_sets.len(), 2);
        // A save into a directory that does not exist fails and leaves nothing behind.
        let missing = dir.join("missing").join("cat.ink3d");
        assert!(save(&missing, &project, &mut ImageCache::default()).is_err());
        assert!(!dir.join("cat.ink3d.saving").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
