//! Sparse storage for painted pixels. Paint layers and masks hold only the tiles that were
//! painted; the tiles live in shared pools, one texture array per format, whose layers
//! ("pages") are filled with tiles side by side.

use egui_wgpu::wgpu;

/// Side of a tile in texels. Divides every texture set resolution.
pub(crate) const TILE: u32 = 128;
/// Side of a pool page in texels.
const PAGE: u32 = 2048;
/// Tiles per page side and per page.
pub(crate) const PAGE_TILES: u32 = PAGE / TILE;
const TILES_PER_PAGE: u32 = PAGE_TILES * PAGE_TILES;
/// Marks a tile that holds no paint.
pub(crate) const EMPTY: u32 = u32::MAX;

/// Where a slot lies in its pool: the page (array layer) and the texel origin there.
pub(crate) fn slot_place(slot: u32) -> (u32, [u32; 2]) {
    let page = slot / TILES_PER_PAGE;
    let i = slot % TILES_PER_PAGE;
    (page, [(i % PAGE_TILES) * TILE, (i / PAGE_TILES) * TILE])
}

/// A growable array of tiles of one format.
pub(crate) struct TilePool {
    label: &'static str,
    format: wgpu::TextureFormat,
    texture: wgpu::Texture,
    /// All pages, for sampling.
    view: wgpu::TextureView,
    /// One view per page, for rendering into it.
    page_views: Vec<wgpu::TextureView>,
    free: Vec<u32>,
    /// Bumped whenever the texture is replaced, so bind groups that use it are remade.
    pub generation: u64,
}

impl TilePool {
    pub fn new(device: &wgpu::Device, label: &'static str, format: wgpu::TextureFormat) -> Self {
        let (texture, view, page_views) = Self::create(device, label, format, 1);
        Self {
            label,
            format,
            texture,
            view,
            page_views,
            free: (0..TILES_PER_PAGE).rev().collect(),
            generation: 0,
        }
    }

    fn create(
        device: &wgpu::Device,
        label: &str,
        format: wgpu::TextureFormat,
        pages: u32,
    ) -> (wgpu::Texture, wgpu::TextureView, Vec<wgpu::TextureView>) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: PAGE,
                height: PAGE,
                depth_or_array_layers: pages,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let page_views = (0..pages)
            .map(|layer| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        (texture, view, page_views)
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn page_view(&self, page: u32) -> &wgpu::TextureView {
        &self.page_views[page as usize]
    }

    pub fn pages(&self) -> u32 {
        self.page_views.len() as u32
    }

    pub fn bytes_per_tile(&self) -> u64 {
        let texel = self.format.block_copy_size(None).unwrap_or(8) as u64;
        texel * (TILE * TILE) as u64
    }

    /// Takes a free slot, growing the pool if it is full. Its texels are undefined.
    pub fn allocate(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> u32 {
        if let Some(slot) = self.free.pop() {
            return slot;
        }
        self.grow(device, queue);
        self.free.pop().expect("a grown pool has free slots")
    }

    /// Grows the pool until `count` slots are free, so that allocating them doesn't replace
    /// the texture while commands that use it are being recorded.
    pub fn reserve(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, count: u32) {
        while (self.free.len() as u32) < count {
            self.grow(device, queue);
        }
    }

    pub fn release(&mut self, slot: u32) {
        debug_assert!(!self.free.contains(&slot));
        self.free.push(slot);
    }

    /// Doubles the number of pages, keeping the tiles where they are.
    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let old_pages = self.pages();
        let limit = device.limits().max_texture_array_layers;
        let pages = (old_pages * 2).min(limit);
        assert!(pages > old_pages, "paint tile pool {} is full", self.label);
        let (texture, view, page_views) = Self::create(device, self.label, self.format, pages);
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_texture(
            self.texture.as_image_copy(),
            texture.as_image_copy(),
            wgpu::Extent3d {
                width: PAGE,
                height: PAGE,
                depth_or_array_layers: old_pages,
            },
        );
        queue.submit([encoder.finish()]);
        self.texture = texture;
        self.view = view;
        self.page_views = page_views;
        // Lowest slots first.
        self.free
            .extend((old_pages * TILES_PER_PAGE..pages * TILES_PER_PAGE).rev());
        self.free.sort_unstable_by(|a, b| b.cmp(a));
        self.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_fill_pages_row_by_row() {
        assert_eq!(slot_place(0), (0, [0, 0]));
        assert_eq!(slot_place(1), (0, [TILE, 0]));
        assert_eq!(slot_place(PAGE_TILES), (0, [0, TILE]));
        assert_eq!(slot_place(TILES_PER_PAGE + 2), (1, [2 * TILE, 0]));
        assert_eq!(PAGE % TILE, 0);
        // Every texture set resolution is a multiple of the tile size.
        for resolution in [128, 256, 512, 1024, 2048, 4096, 8192] {
            assert_eq!(resolution % TILE, 0);
        }
    }
}
