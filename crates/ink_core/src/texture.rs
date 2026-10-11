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

/// Images are identified by their id.
impl PartialEq for TextureImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
