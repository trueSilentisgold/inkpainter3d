//! Brush presets: a brush's settings and alpha in one file (`.inkbrush`, a ZIP of
//! `brush.json` and, for image alphas, `alpha.png`). The shelf lists these files.

use crate::project::{
    FileBrushMaterial, decode, encode_alpha, finite, material_from_file, material_to_file,
};
use anyhow::{Context, Result, bail, ensure};
use ink_core::{
    Alignment, Alpha, AlphaImage, Brush, Jitter, PressureCurve, PressureMapping, SizeSpace,
};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

/// File extension of brush presets.
pub const EXTENSION: &str = "inkbrush";

const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct FilePreset {
    version: u32,
    name: String,
    size: f32,
    size_space: String,
    flow: f32,
    opacity: f32,
    spacing: f32,
    angle: f32,
    hardness: f32,
    follow_path: bool,
    jitter: [f32; 4],
    alignment: String,
    /// `round`, `square` or `image` (then `alpha.png` holds the tip).
    alpha: String,
    #[serde(default)]
    alpha_name: String,
    backface_culling: bool,
    backface_angle: f32,
    pressure: FilePressure,
    material: FileBrushMaterial,
}

#[derive(Serialize, Deserialize)]
struct FilePressure {
    size: bool,
    flow: bool,
    min_size: f32,
    min_flow: f32,
    curve: String,
}

fn size_space_key(s: SizeSpace) -> &'static str {
    match s {
        SizeSpace::Object => "object",
        SizeSpace::Viewport => "viewport",
    }
}

fn curve_key(c: PressureCurve) -> &'static str {
    match c {
        PressureCurve::Linear => "linear",
        PressureCurve::EaseIn => "ease_in",
        PressureCurve::EaseInOut => "ease_in_out",
    }
}

/// Writes `brush` as a preset called `name`.
pub fn save(path: &Path, name: &str, brush: &Brush) -> Result<()> {
    let (alpha_key, alpha_name, png) = match &brush.alpha {
        Alpha::Round => ("round", String::new(), None),
        Alpha::Square => ("square", String::new(), None),
        Alpha::Image(image) => ("image", image.name.clone(), Some(encode_alpha(image)?)),
    };
    let file = FilePreset {
        version: VERSION,
        name: name.to_owned(),
        size: finite(brush.size),
        size_space: size_space_key(brush.size_space).to_owned(),
        flow: finite(brush.flow),
        opacity: finite(brush.opacity),
        spacing: finite(brush.spacing),
        angle: finite(brush.angle),
        hardness: finite(brush.hardness),
        follow_path: brush.follow_path,
        jitter: [
            brush.jitter.size,
            brush.jitter.flow,
            brush.jitter.angle,
            brush.jitter.position,
        ]
        .map(finite),
        alignment: brush.alignment.key().to_owned(),
        alpha: alpha_key.to_owned(),
        alpha_name,
        backface_culling: brush.backface_culling,
        backface_angle: finite(brush.backface_angle),
        pressure: FilePressure {
            size: brush.pressure.size,
            flow: brush.pressure.flow,
            min_size: finite(brush.pressure.min_size),
            min_flow: finite(brush.pressure.min_flow),
            curve: curve_key(brush.pressure.curve).to_owned(),
        },
        material: material_to_file(&brush.material),
    };
    let mut temp_name = path.file_name().unwrap_or_default().to_owned();
    temp_name.push(".saving");
    let temp = path.with_file_name(temp_name);
    let result = (|| -> Result<()> {
        let out = std::fs::File::create(&temp)
            .with_context(|| format!("Failed to create {}", temp.display()))?;
        let mut zip = zip::ZipWriter::new(out);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("brush.json", options)?;
        zip.write_all(&serde_json::to_vec_pretty(&file)?)?;
        if let Some(png) = png {
            zip.start_file("alpha.png", options)?;
            zip.write_all(&png.bytes)?;
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

/// Reads a preset: its name and brush.
pub fn load(path: &Path) -> Result<(String, Brush)> {
    let data = std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    read(&data).with_context(|| format!("{} is not a brush preset", path.display()))
}

fn read(data: &[u8]) -> Result<(String, Brush)> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data))?;
    let file: FilePreset = {
        let mut json = String::new();
        zip.by_name("brush.json")?.read_to_string(&mut json)?;
        serde_json::from_str(&json)?
    };
    ensure!(
        file.version <= VERSION,
        "It was made by a newer version of inkpainter3d"
    );
    let alpha = match file.alpha.as_str() {
        "round" => Alpha::Round,
        "square" => Alpha::Square,
        "image" => {
            let mut bytes = Vec::new();
            zip.by_name("alpha.png")
                .context("The alpha image is missing")?
                .read_to_end(&mut bytes)?;
            let image = decode(&bytes, image::ImageFormat::Png)?.into_luma8();
            let (w, h) = image.dimensions();
            Alpha::Image(Arc::new(AlphaImage::new(
                file.alpha_name.clone(),
                w,
                h,
                image.into_raw(),
            )))
        }
        other => bail!("Unknown brush tip {other:?}"),
    };
    let [js, jf, ja, jp] = file.jitter;
    let brush = Brush {
        size: file.size,
        size_space: match file.size_space.as_str() {
            "viewport" => SizeSpace::Viewport,
            _ => SizeSpace::Object,
        },
        flow: file.flow,
        opacity: file.opacity,
        spacing: file.spacing,
        angle: file.angle,
        hardness: file.hardness,
        follow_path: file.follow_path,
        jitter: Jitter {
            size: js,
            flow: jf,
            angle: ja,
            position: jp,
        },
        alignment: Alignment::from_key(&file.alignment).unwrap_or_default(),
        alpha,
        backface_culling: file.backface_culling,
        backface_angle: file.backface_angle,
        pressure: PressureMapping {
            size: file.pressure.size,
            flow: file.pressure.flow,
            min_size: file.pressure.min_size,
            min_flow: file.pressure.min_flow,
            curve: match file.pressure.curve.as_str() {
                "linear" => PressureCurve::Linear,
                "ease_in_out" => PressureCurve::EaseInOut,
                _ => PressureCurve::EaseIn,
            },
        },
        material: material_from_file(file.material),
    };
    Ok((file.name, brush))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip() {
        let mut brush = Brush {
            size: 33.0,
            size_space: SizeSpace::Viewport,
            flow: 0.4,
            follow_path: true,
            alignment: Alignment::Camera,
            jitter: Jitter {
                size: 0.1,
                flow: 0.2,
                angle: 0.3,
                position: 0.4,
            },
            alpha: Alpha::Image(Arc::new(AlphaImage::new(
                "dots".into(),
                2,
                2,
                vec![0, 255, 128, 7],
            ))),
            ..Brush::default()
        };
        brush.pressure.curve = PressureCurve::Linear;
        brush.material.values.roughness = 0.77;
        let path = std::env::temp_dir().join(format!("ink-preset-{}.inkbrush", std::process::id()));
        save(&path, "Test brush", &brush).unwrap();
        let (name, back) = load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(name, "Test brush");
        assert_eq!(back.size, 33.0);
        assert_eq!(back.size_space, SizeSpace::Viewport);
        assert!(back.follow_path);
        assert_eq!(back.alignment, Alignment::Camera);
        assert_eq!(back.jitter, brush.jitter);
        assert_eq!(back.pressure.curve, PressureCurve::Linear);
        assert_eq!(back.material, brush.material);
        let (Alpha::Image(a), Alpha::Image(b)) = (&brush.alpha, &back.alpha) else {
            panic!("the alpha image is lost");
        };
        assert_eq!((a.name.as_str(), &a.pixels), (b.name.as_str(), &b.pixels));
    }

    #[test]
    fn other_files_are_refused() {
        assert!(read(b"not a zip").is_err());
    }
}
