//! Images that fill layers and fill effects project onto the model.

use std::sync::atomic::{AtomicU64, Ordering};

/// Pixels of a [`TextureImage`], row by row from the top, four components per pixel.
#[derive(Debug)]
pub enum Texels {
    /// 8 bits per component, as most PNG and JPEG files: sRGB-encoded where the image is a
    /// color, raw data (roughness, height, normals) otherwise.
    Rgba8(Vec<u8>),
    /// 16 bits per component, e.g. 16-bit PNG height maps; encoded like [`Self::Rgba8`].
    Rgba16(Vec<u16>),
    /// Linear floats, from HDR and EXR files.
    RgbaF32(Vec<f32>),
}

/// An image loaded from a file.
#[derive(Debug)]
pub struct TextureImage {
    /// Unique within the running program; lets the renderer upload each image once.
    pub id: u64,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub texels: Texels,
}

impl TextureImage {
    pub fn new(name: String, width: u32, height: u32, texels: Texels) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let len = (width * height * 4) as usize;
        match &texels {
            Texels::Rgba8(v) => assert_eq!(v.len(), len),
            Texels::Rgba16(v) => assert_eq!(v.len(), len),
            Texels::RgbaF32(v) => assert_eq!(v.len(), len),
        }
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            name,
            width,
            height,
            texels,
        }
    }
}

impl TextureImage {
    /// The texel nearest to `uv` (origin top-left, wrapping), its components as stored:
    /// 0 to 1 for 8 and 16 bits, raw for floats.
    pub fn texel_at_uv(&self, uv: [f32; 2]) -> [f32; 4] {
        let wrap = |v: f32, size: u32| {
            let t = (v - v.floor()) * size as f32;
            (t as u32).min(size.saturating_sub(1))
        };
        let (x, y) = (wrap(uv[0], self.width), wrap(uv[1], self.height));
        let i = ((y * self.width + x) * 4) as usize;
        match &self.texels {
            Texels::Rgba8(v) => std::array::from_fn(|c| v[i + c] as f32 / 255.0),
            Texels::Rgba16(v) => std::array::from_fn(|c| v[i + c] as f32 / 65535.0),
            Texels::RgbaF32(v) => std::array::from_fn(|c| v[i + c]),
        }
    }
}

/// Images are identified by their id.
impl PartialEq for TextureImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texels_are_read_nearest_and_wrap() {
        // 2 × 1: black, then white.
        let image = TextureImage::new(
            "test".into(),
            2,
            1,
            Texels::Rgba8(vec![0, 0, 0, 255, 255, 255, 255, 255]),
        );
        assert_eq!(image.texel_at_uv([0.25, 0.5]), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(image.texel_at_uv([0.75, 0.5]), [1.0; 4]);
        assert_eq!(image.texel_at_uv([1.25, 0.5]), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(image.texel_at_uv([1.0, 0.5]), [0.0, 0.0, 0.0, 1.0]);
    }
}
