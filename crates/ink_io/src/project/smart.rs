//! Smart materials and smart masks: layers (with their masks and effects) or just a mask,
//! saved as one file (`.inksmart`) the shelf lists and applies to any project. Painted strokes
//! are not part of a preset; everything that comes from the model's mesh maps adapts to the
//! model it is applied to.

use super::*;
use ink_core::LayerKind;

/// File extension of smart presets.
pub const EXTENSION: &str = "inksmart";

const SMART_FORMAT: &str = "inkpainter3d smart";
const SMART_JSON: &str = "smart.json";
const SMART_PREVIEW: &str = "preview.png";

/// What a preset holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmartKind {
    /// Layers, usually a folder of fill layers with masks.
    Material,
    /// A mask with its effects.
    Mask,
}

impl SmartKind {
    fn key(self) -> &'static str {
        match self {
            Self::Material => "material",
            Self::Mask => "mask",
        }
    }
}

/// A preset.
#[derive(Clone, Debug)]
pub struct Smart {
    pub name: String,
    pub kind: SmartKind,
    /// The layers of a material, bottom to top. A mask preset is one layer that only carries
    /// the mask.
    pub layers: Vec<Layer>,
    /// A color that stands for the preset, if its layers don't say it well.
    pub swatch_color: Option<[u8; 3]>,
}

impl Smart {
    /// A material preset from layers.
    pub fn material(name: String, layers: Vec<Layer>) -> Self {
        Self {
            name,
            kind: SmartKind::Material,
            layers,
            swatch_color: None,
        }
    }

    /// The color shown for the preset.
    pub fn with_swatch(mut self, color: [f32; 3]) -> Self {
        self.swatch_color = Some(color.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
        self
    }

    /// A mask preset.
    pub fn mask(name: String, mask: Mask, ids: &mut Ids) -> Self {
        let mut carrier = Layer::new(LayerKind::Fill, name.clone(), ids);
        carrier.mask = Some(mask);
        Self {
            name,
            kind: SmartKind::Mask,
            layers: vec![carrier],
            swatch_color: None,
        }
    }

    /// The mask of a mask preset.
    pub fn mask_of(&self) -> Option<&Mask> {
        self.layers.first()?.mask.as_ref()
    }

    /// A color that stands for the preset: the base color of its lowest fill layer without a
    /// mask (the material underneath), or the average of that layer's image.
    pub fn swatch(&self) -> [u8; 3] {
        if let Some(color) = self.swatch_color {
            return color;
        }
        fn find(layers: &[Layer], masked_ok: bool) -> Option<&Fill> {
            for layer in layers {
                match &layer.content {
                    Content::Fill(fill)
                        if fill.channels.contains(Channel::BaseColor)
                            && (masked_ok || layer.mask.is_none()) =>
                    {
                        return Some(fill);
                    }
                    Content::Folder(folder) => {
                        if let Some(fill) = find(&folder.layers, masked_ok) {
                            return Some(fill);
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        let found = find(&self.layers, false).or_else(|| find(&self.layers, true));
        let Some(fill) = found else {
            return [0x80; 3];
        };
        if let Some(image) = fill.image(Channel::BaseColor) {
            let rgba = crate::texture_to_rgba8(image);
            let (mut sum, mut n) = ([0u64; 3], 0u64);
            for p in rgba
                .chunks_exact(4)
                .step_by(((rgba.len() / 4) / 4096).max(1))
            {
                for c in 0..3 {
                    sum[c] += p[c] as u64;
                }
                n += 1;
            }
            if n > 0 {
                return sum.map(|s| (s / n) as u8);
            }
        }
        let [r, g, b] = fill.values.base_color;
        [r, g, b].map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
    }
}

/// Paint layers keep no strokes in a preset.
fn strip_strokes(layers: &mut [Layer]) {
    for layer in layers {
        match &mut layer.content {
            Content::Paint(paint) => paint.strokes.clear(),
            Content::Fill(_) => {}
            Content::Folder(folder) => strip_strokes(&mut folder.layers),
        }
        if let Some(mask) = &mut layer.mask {
            mask.paint.strokes.clear();
            for effect in &mut mask.effects {
                if let EffectKind::Paint(paint) = &mut effect.kind {
                    paint.strokes.clear();
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
struct FileSmart {
    format: String,
    version: u32,
    name: String,
    kind: String,
    layers: Vec<FileLayer>,
    #[serde(default)]
    images: Vec<FileImage>,
    #[serde(default)]
    alphas: Vec<FileAlpha>,
}

/// Writes a preset.
pub fn save(path: &Path, smart: &Smart) -> Result<()> {
    let mut layers = smart.layers.clone();
    strip_strokes(&mut layers);
    let mut saver = Saver::default();
    let file_layers: Vec<FileLayer> = layers.iter().map(|l| saver.layer(l)).collect();
    let images: Vec<FileImage> = saver
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
    let alphas: Vec<FileAlpha> = saver
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
    let file = FileSmart {
        format: SMART_FORMAT.to_owned(),
        version: 1,
        name: smart.name.clone(),
        kind: smart.kind.key().to_owned(),
        layers: file_layers,
        images,
        alphas,
    };

    let mut temp_name = path.file_name().unwrap_or_default().to_owned();
    temp_name.push(".saving");
    let temp = path.with_file_name(temp_name);
    let result = (|| -> Result<()> {
        let out = std::fs::File::create(&temp)
            .with_context(|| format!("Failed to create {}", temp.display()))?;
        let mut zip = zip::ZipWriter::new(out);
        let deflated = zip::write::SimpleFileOptions::default();
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .large_file(true);
        zip.start_file(SMART_JSON, deflated)?;
        zip.write_all(&serde_json::to_vec_pretty(&file)?)?;
        // A swatch as the preview: small, so listing the shelf never reads the images.
        let [r, g, b] = smart.swatch();
        let mut preview = Vec::new();
        {
            use image::ImageEncoder;
            let pixels: Vec<u8> = (0..16 * 16).flat_map(|_| [r, g, b, 255]).collect();
            png_encoder(&mut preview).write_image(
                &pixels,
                16,
                16,
                image::ExtendedColorType::Rgba8,
            )?;
        }
        zip.start_file(SMART_PREVIEW, stored)?;
        zip.write_all(&preview)?;
        for (image, entry) in saver.textures.iter().zip(&file.images) {
            zip.start_file(&entry.file, stored)?;
            zip.write_all(&encode_texture(image)?.bytes)?;
        }
        for (alpha, entry) in saver.alphas.iter().zip(&file.alphas) {
            zip.start_file(&entry.file, stored)?;
            zip.write_all(&encode_alpha(alpha)?.bytes)?;
        }
        zip.finish()?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn open(path: &Path) -> Result<zip::ZipArchive<std::io::BufReader<std::fs::File>>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("Failed to read {}", path.display()))?;
    zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("{} is not a smart preset", path.display()))
}

fn read_file(zip: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>) -> Result<FileSmart> {
    let json = read_entry(zip, SMART_JSON, 1 << 28)?;
    let file: FileSmart = serde_json::from_slice(&json).context("Not a smart preset")?;
    ensure!(file.format == SMART_FORMAT, "Not a smart preset");
    ensure!(
        file.version <= 1,
        "It was made by a newer version of inkpainter3d"
    );
    Ok(file)
}

/// The name, kind and swatch of a preset, without reading its images.
pub fn preview(path: &Path) -> Result<(String, SmartKind, [u8; 3])> {
    let mut zip = open(path)?;
    let file = read_file(&mut zip)?;
    let kind = match file.kind.as_str() {
        "mask" => SmartKind::Mask,
        _ => SmartKind::Material,
    };
    let bytes = read_entry(&mut zip, SMART_PREVIEW, 1 << 20)?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?.into_rgba8();
    let p = image.get_pixel(0, 0).0;
    Ok((file.name, kind, [p[0], p[1], p[2]]))
}

/// Reads a preset.
pub fn load(path: &Path) -> Result<Smart> {
    let mut zip = open(path)?;
    let file = read_file(&mut zip)?;
    let mut textures = Vec::with_capacity(file.images.len());
    for entry in &file.images {
        check_size(entry.width, entry.height, crate::MAX_TEXTURE_SIZE)?;
        let bytes = read_entry(
            &mut zip,
            &entry.file,
            encoded_limit(entry.width, entry.height),
        )?;
        textures.push(Arc::new(decode_texture(entry, &bytes).with_context(
            || format!("Failed to read the image {}", entry.file),
        )?));
    }
    let mut alphas = Vec::with_capacity(file.alphas.len());
    for entry in &file.alphas {
        check_size(entry.width, entry.height, crate::MAX_ALPHA_SIZE)?;
        let bytes = read_entry(
            &mut zip,
            &entry.file,
            encoded_limit(entry.width, entry.height),
        )?;
        alphas.push(Arc::new(decode_alpha(entry, &bytes).with_context(
            || format!("Failed to read the alpha {}", entry.file),
        )?));
    }
    let highest = highest_id(&file.layers);
    ensure!(highest < MAX_ID, "The preset's ids are damaged");
    let mut loader = Loader {
        dabs: &[],
        next_dab: 0,
        textures: &textures,
        alphas: &alphas,
        ids: HashSet::new(),
        next_id: highest + 1,
    };
    let layers = loader.layers(file.layers)?;
    Ok(Smart {
        name: file.name,
        kind: match file.kind.as_str() {
            "mask" => SmartKind::Mask,
            _ => SmartKind::Material,
        },
        layers,
        swatch_color: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_come_back_without_strokes() {
        let mut ids = Ids::default();
        let mut fill = Layer::new(LayerKind::Fill, "Rust".into(), &mut ids);
        if let Content::Fill(f) = &mut fill.content {
            f.values.base_color = [0.8, 0.4, 0.1];
        }
        let mut mask = Mask::new(0.0, ids.next());
        mask.effects.push(Effect::new(
            ids.next(),
            EffectKind::Generator(Generator::new(GeneratorKind::Dirt)),
        ));
        fill.mask = Some(mask);
        let mut paint = Layer::new(LayerKind::Paint, "Paint".into(), &mut ids);
        if let Content::Paint(p) = &mut paint.content {
            p.strokes.push(Arc::new(Stroke {
                id: 1,
                tool: Tool::Paint,
                space: StrokeSpace::Uv,
                alpha: Alpha::Round,
                hardness: 1.0,
                alignment: Alignment::default(),
                opacity: 1.0,
                backface_cos: -1.0,
                material: BrushMaterial::default(),
                camera: None,
                hidden_objects: Vec::new(),
                hidden_sets: Vec::new(),
                fill: None,
                stencil: None,
                dabs: Vec::new(),
            }));
        }
        let mut folder = Layer::new(LayerKind::Folder, "Worn".into(), &mut ids);
        if let Content::Folder(f) = &mut folder.content {
            f.layers = vec![fill, paint];
        }
        let smart = Smart::material("Worn".into(), vec![folder]);
        assert_eq!(smart.swatch(), [204, 102, 26]);

        let path = std::env::temp_dir().join(format!("ink-smart-{}.inksmart", std::process::id()));
        save(&path, &smart).unwrap();
        let (name, kind, swatch) = preview(&path).unwrap();
        assert_eq!(
            (name.as_str(), kind, swatch),
            ("Worn", SmartKind::Material, [204, 102, 26])
        );
        let back = load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let Content::Folder(folder) = &back.layers[0].content else {
            panic!("a folder")
        };
        assert_eq!(folder.layers.len(), 2);
        let Content::Paint(p) = &folder.layers[1].content else {
            panic!("a paint layer")
        };
        assert!(p.strokes.is_empty(), "strokes are not part of a preset");
        let mask = folder.layers[0].mask.as_ref().unwrap();
        assert!(matches!(
            &mask.effects[0].kind,
            EffectKind::Generator(g) if g.kind == GeneratorKind::Dirt
        ));
    }

    #[test]
    fn masks_are_presets_too() {
        let mut ids = Ids::default();
        let mask = Mask::new(1.0, ids.next());
        let smart = Smart::mask("White".into(), mask, &mut ids);
        let path =
            std::env::temp_dir().join(format!("ink-smart-mask-{}.inksmart", std::process::id()));
        save(&path, &smart).unwrap();
        let back = load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(back.kind, SmartKind::Mask);
        assert_eq!(back.mask_of().unwrap().background, 1.0);
    }
}
