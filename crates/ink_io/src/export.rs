//! Writes exported texture maps to files.

use anyhow::{Context, Result, bail};
use ink_core::export::{BitDepth, FileFormat, MapImage, MapPixels};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// JPEG quality of exported maps, 1 to 100.
const JPEG_QUALITY: u8 = 95;

/// A map to write: where and what.
pub struct MapFile {
    pub path: PathBuf,
    pub image: MapImage,
}

/// Writes the maps, several at a time, and reports each one as it is done.
pub fn write_maps(
    maps: Vec<MapFile>,
    format: FileFormat,
    bit_depth: BitDepth,
    done: impl Fn(&Path, Result<()>) + Sync,
) {
    let threads = std::thread::available_parallelism()
        .map_or(2, |n| n.get())
        .min(maps.len().max(1));
    let queue = std::sync::Mutex::new(maps);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let Some(map) = queue.lock().unwrap_or_else(|e| e.into_inner()).pop() else {
                        break;
                    };
                    let result = write_map(&map.path, &map.image, format, bit_depth);
                    done(&map.path, result);
                }
            });
        }
    });
}

/// Writes one map. The file is replaced only once the new one is complete, so a failed
/// write leaves the old file (or none) and never half a file.
pub fn write_map(
    path: &Path,
    image: &MapImage,
    format: FileFormat,
    bit_depth: BitDepth,
) -> Result<()> {
    let mut temp_name = path.file_name().unwrap_or_default().to_owned();
    temp_name.push(".part");
    let temp = path.with_file_name(temp_name);
    let result = (|| -> Result<()> {
        let file = std::fs::File::create(&temp)?;
        let mut writer = BufWriter::new(file);
        encode(&mut writer, image, format, bit_depth)?;
        writer.flush()?;
        drop(writer);
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.with_context(|| format!("Failed to write {}", path.display()))
}

/// Encodes a map in the file format.
pub fn encode<W: Write + std::io::Seek>(
    writer: W,
    image: &MapImage,
    format: FileFormat,
    bit_depth: BitDepth,
) -> Result<()> {
    use image::ExtendedColorType as C;
    use image::ImageEncoder;
    let (w, h) = (image.width, image.height);
    let n = image.components;
    match (&image.pixels, format) {
        (MapPixels::F32(pixels), FileFormat::OpenExr) => {
            return encode_exr(writer, w, h, n, pixels, bit_depth);
        }
        (MapPixels::F32(_), _) | (_, FileFormat::OpenExr) => {
            bail!("{} files can't hold {}", format.label(), bit_depth.label())
        }
        (MapPixels::U8(pixels), _) => {
            let (pixels, n) = expand(pixels, n, format);
            let color = [C::L8, C::La8, C::Rgb8, C::Rgba8][n - 1];
            match format {
                FileFormat::Png => png(writer).write_image(&pixels, w, h, color)?,
                FileFormat::Targa => {
                    image::codecs::tga::TgaEncoder::new(writer).write_image(&pixels, w, h, color)?
                }
                FileFormat::Jpeg => {
                    image::codecs::jpeg::JpegEncoder::new_with_quality(writer, JPEG_QUALITY)
                        .write_image(&pixels, w, h, color)?
                }
                FileFormat::Tiff => image::codecs::tiff::TiffEncoder::new(writer)
                    .write_image(&pixels, w, h, color)?,
                FileFormat::OpenExr => unreachable!(),
            }
        }
        (MapPixels::U16(pixels), _) => {
            let (pixels, n) = expand(pixels, n, format);
            let color = [C::L16, C::La16, C::Rgb16, C::Rgba16][n - 1];
            let bytes: Vec<u8> = pixels.iter().flat_map(|v| v.to_ne_bytes()).collect();
            match format {
                FileFormat::Png => png(writer).write_image(&bytes, w, h, color)?,
                FileFormat::Tiff => image::codecs::tiff::TiffEncoder::new(writer)
                    .write_image(&bytes, w, h, color)?,
                _ => bail!("{} files can't hold 16 bits", format.label()),
            }
        }
    }
    Ok(())
}

fn png<W: Write>(writer: W) -> image::codecs::png::PngEncoder<W> {
    image::codecs::png::PngEncoder::new_with_quality(
        writer,
        image::codecs::png::CompressionType::Default,
        image::codecs::png::FilterType::Adaptive,
    )
}

/// Turns pixel layouts the format can't store into ones it can: gray with alpha into RGBA
/// for TIFF, and drops alpha for JPEG.
fn expand<T: Copy>(
    pixels: &[T],
    n: usize,
    format: FileFormat,
) -> (std::borrow::Cow<'_, [T]>, usize) {
    match (format, n) {
        (FileFormat::Tiff, 2) => (
            pixels
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|&[l, a]| [l, l, l, a])
                .collect(),
            4,
        ),
        (FileFormat::Jpeg, 2) => (pixels.iter().step_by(2).copied().collect(), 1),
        (FileFormat::Jpeg, 4) => (
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[r, g, b, _]| [r, g, b])
                .collect(),
            3,
        ),
        _ => (pixels.into(), n),
    }
}

fn encode_exr<W: Write + std::io::Seek>(
    writer: W,
    width: u32,
    height: u32,
    n: usize,
    pixels: &[f32],
    bit_depth: BitDepth,
) -> Result<()> {
    use exr::prelude::*;
    // Gray maps get three equal color channels: not every program reads luminance-only
    // EXR files.
    let (names, sources): (&[&str], &[usize]) = match n {
        1 => (&["R", "G", "B"], &[0, 0, 0]),
        2 => (&["R", "G", "B", "A"], &[0, 0, 0, 1]),
        3 => (&["R", "G", "B"], &[0, 1, 2]),
        _ => (&["R", "G", "B", "A"], &[0, 1, 2, 3]),
    };
    let channels: SmallVec<[AnyChannel<FlatSamples>; 4]> = names
        .iter()
        .zip(sources)
        .map(|(name, &c)| {
            let values = pixels.iter().skip(c).step_by(n).copied();
            let samples = if bit_depth == BitDepth::Float32 {
                FlatSamples::F32(values.collect())
            } else {
                FlatSamples::F16(values.map(f16::from_f32).collect())
            };
            AnyChannel::new(*name, samples)
        })
        .collect();
    let image = Image::from_encoded_channels(
        (width as usize, height as usize),
        Encoding::SMALL_LOSSLESS,
        AnyChannels::sort(channels),
    );
    image
        .write()
        .to_buffered(writer)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn image(components: usize, pixels: MapPixels) -> MapImage {
        MapImage {
            width: 2,
            height: 1,
            components,
            pixels,
        }
    }

    fn round_trip(map: &MapImage, format: FileFormat, depth: BitDepth) -> image::DynamicImage {
        let mut bytes = Vec::new();
        encode(Cursor::new(&mut bytes), map, format, depth).unwrap();
        let format = match format {
            FileFormat::Png => image::ImageFormat::Png,
            FileFormat::Targa => image::ImageFormat::Tga,
            FileFormat::Jpeg => image::ImageFormat::Jpeg,
            FileFormat::Tiff => image::ImageFormat::Tiff,
            FileFormat::OpenExr => image::ImageFormat::OpenExr,
        };
        image::load_from_memory_with_format(&bytes, format).unwrap()
    }

    #[test]
    fn writes_every_format_and_depth() {
        let rgba8 = image(4, MapPixels::U8(vec![10, 20, 30, 40, 50, 60, 70, 80]));
        for format in [FileFormat::Png, FileFormat::Targa, FileFormat::Tiff] {
            let back = round_trip(&rgba8, format, BitDepth::Eight).into_rgba8();
            assert_eq!(
                back.as_raw(),
                &[10, 20, 30, 40, 50, 60, 70, 80],
                "{format:?}"
            );
        }
        let jpeg = round_trip(&rgba8, FileFormat::Jpeg, BitDepth::Eight);
        assert_eq!(jpeg.color(), image::ColorType::Rgb8, "JPEG drops alpha");

        let gray16 = image(1, MapPixels::U16(vec![1, 65000]));
        for format in [FileFormat::Png, FileFormat::Tiff] {
            let back = round_trip(&gray16, format, BitDepth::Sixteen).into_luma16();
            assert_eq!(back.as_raw(), &[1, 65000], "{format:?}");
        }
        let gray_alpha = image(2, MapPixels::U8(vec![100, 255, 7, 0]));
        let tiff = round_trip(&gray_alpha, FileFormat::Tiff, BitDepth::Eight).into_rgba8();
        assert_eq!(tiff.as_raw(), &[100, 100, 100, 255, 7, 7, 7, 0]);
        assert!(
            round_trip(&gray_alpha, FileFormat::Png, BitDepth::Eight)
                .color()
                .has_alpha()
        );

        let floats = image(3, MapPixels::F32(vec![0.25, 2.0, -1.0, 0.5, 0.0, 1e-3]));
        for depth in [BitDepth::Float16, BitDepth::Float32] {
            let back = round_trip(&floats, FileFormat::OpenExr, depth).into_rgb32f();
            for (a, b) in back.as_raw().iter().zip([0.25, 2.0, -1.0, 0.5, 0.0, 1e-3]) {
                assert!((a - b).abs() < 1e-3, "{depth:?}: {a} vs {b}");
            }
        }
        let gray_float = image(1, MapPixels::F32(vec![0.75, 0.125]));
        let back = round_trip(&gray_float, FileFormat::OpenExr, BitDepth::Float16);
        assert!((back.into_rgb32f().as_raw()[3] - 0.125).abs() < 1e-4);
    }

    #[test]
    fn refuses_depths_the_format_cant_hold() {
        let gray16 = image(1, MapPixels::U16(vec![1, 2]));
        let mut bytes = Vec::new();
        assert!(
            encode(
                Cursor::new(&mut bytes),
                &gray16,
                FileFormat::Jpeg,
                BitDepth::Sixteen
            )
            .is_err()
        );
    }

    #[test]
    fn written_files_go_where_asked() {
        let dir = std::env::temp_dir().join(format!("ink_export_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let maps: Vec<MapFile> = (0..5)
            .map(|i| MapFile {
                path: dir.join(format!("map{i}.png")),
                image: image(1, MapPixels::U8(vec![i, i])),
            })
            .collect();
        let written = std::sync::Mutex::new(Vec::new());
        write_maps(maps, FileFormat::Png, BitDepth::Eight, |path, result| {
            result.unwrap();
            written.lock().unwrap().push(path.to_owned());
        });
        assert_eq!(written.into_inner().unwrap().len(), 5);
        let back = image::open(dir.join("map3.png")).unwrap().into_luma8();
        assert_eq!(back.as_raw(), &[3, 3]);
        // A failed write leaves the old file as it was.
        let readonly = dir.join("map3.png");
        let invalid = image(1, MapPixels::U16(vec![1, 2]));
        assert!(write_map(&readonly, &invalid, FileFormat::Jpeg, BitDepth::Sixteen).is_err());
        assert_eq!(
            image::open(&readonly).unwrap().into_luma8().as_raw(),
            &[3, 3]
        );
        assert!(!dir.join("map3.png.part").exists());
        let missing = dir.join("missing").join("map.png");
        assert!(
            write_map(
                &missing,
                &image(1, MapPixels::U8(vec![0, 0])),
                FileFormat::Png,
                BitDepth::Eight
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
