//! Materials for the shelf: a set of channel maps in one file (`.inkmat`, a ZIP of
//! `material.json`, the maps as they came, and a `preview.png`). They are made by importing
//! a folder of PBR maps, as Poly Haven and ambientCG ship them, or by downloading from
//! those sites.

use crate::{decode_texture, project::png_encoder};
use anyhow::{Context, Result, bail, ensure};
use ink_core::{Channel, TextureImage};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;

/// File extension of shelf materials.
pub const EXTENSION: &str = "inkmat";

/// Side of the preview image stored in a material.
pub const PREVIEW_SIZE: u32 = 192;

const VERSION: u32 = 1;

/// One map of a material, still in its file format.
#[derive(Clone, Debug)]
pub struct MapFile {
    pub channel: Channel,
    /// Name inside the material; its extension tells the image format.
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// A material as files: what an import produces and `save` writes.
#[derive(Clone, Debug)]
pub struct MaterialFiles {
    pub name: String,
    /// Whether the normal map's green channel points down (DirectX).
    pub normal_directx: bool,
    pub maps: Vec<MapFile>,
}

/// A material with decoded images, ready to become a fill layer.
#[derive(Debug)]
pub struct Material {
    pub name: String,
    pub normal_directx: bool,
    pub images: Vec<(Channel, TextureImage)>,
}

#[derive(Serialize, Deserialize)]
struct FileMaterial {
    version: u32,
    name: String,
    normal_directx: bool,
    /// Channel key and file name inside the archive.
    maps: Vec<(String, String)>,
}

/// What a file name says about the map it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Classified {
    pub channel: Channel,
    /// Only for normal maps: whether the name says DirectX.
    pub directx: Option<bool>,
}

/// Works out the channel of a map from its file name, e.g. `Bricks075A_1K-JPG_NormalGL.jpg`
/// or `brick_wall_001_nor_gl_1k.exr`. None for names it does not know (ambient occlusion,
/// packed maps, previews).
pub fn classify(file_name: &str) -> Option<Classified> {
    let stem = Path::new(file_name)
        .file_stem()?
        .to_string_lossy()
        .to_lowercase();
    // Resolution and format tokens (1k, 4k, jpg) carry no meaning for the map.
    let tokens: Vec<&str> = stem
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .filter(|t| {
            let resolution = t
                .strip_suffix('k')
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
            !resolution && !matches!(*t, "jpg" | "jpeg" | "png" | "exr" | "tif" | "tiff" | "tga")
        })
        .collect();
    let last = *tokens.last()?;
    let before = tokens.len().checked_sub(2).map(|i| tokens[i]);
    // Two-token names like nor_gl: look at both.
    let pair = before.map(|b| format!("{b}{last}"));
    let normal = |directx| {
        Some(Classified {
            channel: Channel::Normal,
            directx,
        })
    };
    if let Some(pair) = pair.as_deref() {
        match pair {
            "norgl" | "normalgl" | "nrmgl" => return normal(Some(false)),
            "nordx" | "normaldx" | "nrmdx" => return normal(Some(true)),
            _ => {}
        }
    }
    let plain = |channel| {
        Some(Classified {
            channel,
            directx: None,
        })
    };
    match last {
        "normalgl" => normal(Some(false)),
        "normaldx" => normal(Some(true)),
        "normal" | "nor" | "nrm" | "normalmap" => normal(None),
        "color" | "colour" | "basecolor" | "albedo" | "diffuse" | "diff" | "col" => {
            plain(Channel::BaseColor)
        }
        "displacement" | "disp" | "height" | "bump" => plain(Channel::Height),
        "roughness" | "rough" => plain(Channel::Roughness),
        "metalness" | "metallic" | "metal" => plain(Channel::Metallic),
        "emission" | "emissive" | "emit" => plain(Channel::Emissive),
        "opacity" | "alpha" => plain(Channel::Opacity),
        _ => None,
    }
}

/// Builds a material from files by their names; `name` names the material. Where several
/// files claim a channel, the biggest wins (the highest resolution).
pub fn from_named_files(
    name: &str,
    files: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Result<MaterialFiles> {
    let mut best: Vec<Option<(Classified, String, Vec<u8>)>> = vec![None; Channel::COUNT];
    for (file_name, bytes) in files {
        let Some(class) = classify(&file_name) else {
            continue;
        };
        let slot = &mut best[class.channel.index()];
        let better = match slot {
            None => true,
            Some((old, _, old_bytes)) => {
                // OpenGL normals first (we work in OpenGL), then the larger file.
                let rank = |c: &Classified| u8::from(c.directx != Some(true));
                (rank(&class), bytes.len()) > (rank(old), old_bytes.len())
            }
        };
        if better {
            *slot = Some((class, file_name, bytes));
        }
    }
    let mut normal_directx = false;
    let mut maps = Vec::new();
    for (channel, entry) in Channel::ALL.into_iter().zip(best) {
        let Some((class, file_name, bytes)) = entry else {
            continue;
        };
        if channel == Channel::Normal {
            // A plain "normal" is taken to be OpenGL, as the sites' own are.
            normal_directx = class.directx == Some(true);
        }
        let extension = Path::new(&file_name)
            .extension()
            .map_or("png".to_owned(), |e| e.to_string_lossy().to_lowercase());
        maps.push(MapFile {
            channel,
            file_name: format!("{}.{extension}", channel.key()),
            bytes,
        });
    }
    ensure!(
        !maps.is_empty(),
        "No PBR maps found. Expected files ending in Color, Roughness, Normal and so on."
    );
    Ok(MaterialFiles {
        name: name.to_owned(),
        normal_directx,
        maps,
    })
}

/// Imports a folder of maps as a material named after the folder.
pub fn from_folder(dir: &Path) -> Result<MaterialFiles> {
    let name = dir
        .file_name()
        .map_or("Material".to_owned(), |n| n.to_string_lossy().into_owned());
    let mut files = Vec::new();
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))?
    {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if classify(&file_name).is_some() {
            files.push((
                file_name,
                std::fs::read(&path)
                    .with_context(|| format!("Failed to read {}", path.display()))?,
            ));
        }
    }
    from_named_files(&name, files)
}

fn preview_png(material: &MaterialFiles) -> Result<Vec<u8>> {
    let map = material
        .maps
        .iter()
        .find(|m| m.channel == Channel::BaseColor)
        .or_else(|| material.maps.first())
        .context("The material has no maps")?;
    let texture = decode_texture(&map.file_name, &map.bytes)?;
    let rgba = crate::texture_to_rgba8(&texture);
    let image = image::RgbaImage::from_raw(texture.width, texture.height, rgba)
        .context("Bad preview size")?;
    let small = if texture.width.max(texture.height) <= PREVIEW_SIZE {
        image
    } else {
        image::DynamicImage::ImageRgba8(image)
            .thumbnail(PREVIEW_SIZE, PREVIEW_SIZE)
            .into_rgba8()
    };
    let mut bytes = Vec::new();
    use image::ImageEncoder;
    png_encoder(&mut bytes).write_image(
        small.as_raw(),
        small.width(),
        small.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(bytes)
}

/// Writes the material to `path`.
pub fn save(path: &Path, material: &MaterialFiles) -> Result<()> {
    let preview = preview_png(material)?;
    let file = FileMaterial {
        version: VERSION,
        name: material.name.clone(),
        normal_directx: material.normal_directx,
        maps: material
            .maps
            .iter()
            .map(|m| (m.channel.key().to_owned(), m.file_name.clone()))
            .collect(),
    };
    let mut temp_name = path.file_name().unwrap_or_default().to_owned();
    temp_name.push(".saving");
    let temp = path.with_file_name(temp_name);
    let result = (|| -> Result<()> {
        let out = std::fs::File::create(&temp)
            .with_context(|| format!("Failed to create {}", temp.display()))?;
        let mut zip = zip::ZipWriter::new(out);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .large_file(true);
        zip.start_file("material.json", zip::write::SimpleFileOptions::default())?;
        zip.write_all(&serde_json::to_vec_pretty(&file)?)?;
        zip.start_file("preview.png", stored)?;
        zip.write_all(&preview)?;
        for map in &material.maps {
            zip.start_file(&map.file_name, stored)?;
            zip.write_all(&map.bytes)?;
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
        .with_context(|| format!("{} is not a material", path.display()))
}

fn read_manifest(
    zip: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
) -> Result<FileMaterial> {
    let mut json = String::new();
    zip.by_name("material.json")?.read_to_string(&mut json)?;
    let file: FileMaterial = serde_json::from_str(&json)?;
    ensure!(
        file.version <= VERSION,
        "It was made by a newer version of inkpainter3d"
    );
    Ok(file)
}

/// The name and preview of a material, without reading its maps.
pub fn preview(path: &Path) -> Result<(String, image::RgbaImage)> {
    let mut zip = open(path)?;
    let manifest = read_manifest(&mut zip)?;
    let mut bytes = Vec::new();
    zip.by_name("preview.png")
        .context("The preview is missing")?
        .read_to_end(&mut bytes)?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?.into_rgba8();
    Ok((manifest.name, image))
}

/// Reads a material with all its maps decoded.
pub fn load(path: &Path) -> Result<Material> {
    let mut zip = open(path)?;
    let manifest = read_manifest(&mut zip)?;
    let mut images = Vec::new();
    for (key, file_name) in &manifest.maps {
        let Some(channel) = Channel::from_key(key) else {
            continue;
        };
        let mut bytes = Vec::new();
        zip.by_name(file_name)
            .with_context(|| format!("The map {file_name} is missing"))?
            .read_to_end(&mut bytes)?;
        let mut texture = decode_texture(file_name, &bytes)?;
        texture.name = format!("{} {}", manifest.name, channel.label());
        images.push((channel, texture));
    }
    if images.is_empty() {
        bail!("The material has no maps");
    }
    Ok(Material {
        name: manifest.name,
        normal_directx: manifest.normal_directx,
        images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(name: &str) -> Option<(Channel, Option<bool>)> {
        classify(name).map(|c| (c.channel, c.directx))
    }

    #[test]
    fn names_from_both_sites_are_understood() {
        // ambientCG
        assert_eq!(
            class("Bricks075A_1K-JPG_Color.jpg"),
            Some((Channel::BaseColor, None))
        );
        assert_eq!(
            class("Bricks075A_1K-JPG_NormalGL.jpg"),
            Some((Channel::Normal, Some(false)))
        );
        assert_eq!(
            class("Bricks075A_1K-JPG_NormalDX.jpg"),
            Some((Channel::Normal, Some(true)))
        );
        assert_eq!(
            class("Bricks075A_1K-JPG_Roughness.jpg"),
            Some((Channel::Roughness, None))
        );
        assert_eq!(
            class("Bricks075A_1K-JPG_Displacement.jpg"),
            Some((Channel::Height, None))
        );
        assert_eq!(
            class("Metal032_1K-JPG_Metalness.jpg"),
            Some((Channel::Metallic, None))
        );
        assert_eq!(class("Bricks075A_1K-JPG_AmbientOcclusion.jpg"), None);
        // Poly Haven
        assert_eq!(
            class("brick_wall_001_diff_1k.jpg"),
            Some((Channel::BaseColor, None))
        );
        assert_eq!(
            class("brick_wall_001_nor_gl_1k.exr"),
            Some((Channel::Normal, Some(false)))
        );
        assert_eq!(
            class("brick_wall_001_nor_dx_1k.exr"),
            Some((Channel::Normal, Some(true)))
        );
        assert_eq!(
            class("brick_wall_001_rough_1k.jpg"),
            Some((Channel::Roughness, None))
        );
        assert_eq!(
            class("brick_wall_001_disp_1k.png"),
            Some((Channel::Height, None))
        );
        assert_eq!(class("brick_wall_001_arm_1k.jpg"), None);
        // Others
        assert_eq!(
            class("wood_basecolor.png"),
            Some((Channel::BaseColor, None))
        );
        assert_eq!(class("wood_normal.png"), Some((Channel::Normal, None)));
        // The material's own name never counts.
        assert_eq!(class("Metal_preview.png"), None);
        assert_eq!(class("readme.txt"), None);
    }

    fn png(r: u8, size: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(size, size, image::Rgba([r, 10, 20, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn folders_become_materials_that_come_back() {
        let dir = std::env::temp_dir().join(format!("ink-mat-{}", std::process::id()));
        let source = dir.join("Rusty plate");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("Plate_1K_Color.png"), png(200, 8)).unwrap();
        std::fs::write(source.join("Plate_1K_NormalDX.png"), png(128, 4)).unwrap();
        std::fs::write(source.join("Plate_1K_NormalGL.png"), png(127, 4)).unwrap();
        std::fs::write(source.join("Plate_1K_Roughness.png"), png(90, 8)).unwrap();
        std::fs::write(source.join("notes.txt"), "hi").unwrap();

        let files = from_folder(&source).unwrap();
        assert_eq!(files.name, "Rusty plate");
        assert!(!files.normal_directx, "the OpenGL map wins");
        assert_eq!(files.maps.len(), 3);

        let path = dir.join("plate.inkmat");
        save(&path, &files).unwrap();
        let (name, preview) = preview(&path).unwrap();
        assert_eq!(name, "Rusty plate");
        assert_eq!(preview.dimensions(), (8, 8));
        let material = load(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let channels: Vec<Channel> = material.images.iter().map(|(c, _)| *c).collect();
        assert_eq!(
            channels,
            [Channel::BaseColor, Channel::Roughness, Channel::Normal]
        );
        assert_eq!(material.images[0].1.width, 8);
    }

    #[test]
    fn folders_without_maps_are_refused() {
        assert!(from_named_files("x", [("a.txt".to_owned(), vec![1])]).is_err());
    }
}
