//! Rectangles of texels and regions made of a few of them, for tracking what changed.

use egui_wgpu::wgpu;

/// A rectangle of texels, `min` inclusive, `max` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TexelRect {
    pub min: [u32; 2],
    pub max: [u32; 2],
}

impl TexelRect {
    pub(crate) const EMPTY: Self = Self {
        min: [u32::MAX; 2],
        max: [0; 2],
    };

    pub(crate) fn full(size: u32) -> Self {
        Self {
            min: [0; 2],
            max: [size; 2],
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.min[0] >= self.max[0] || self.min[1] >= self.max[1]
    }

    pub(crate) fn union(self, other: Self) -> Self {
        Self {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
        }
    }

    /// The texels covered by a UV-space box, grown by `margin` and clipped to the texture.
    pub(crate) fn from_uv(min: [f32; 2], max: [f32; 2], size: u32, margin: u32) -> Self {
        let s = size as f32;
        let lo = |v: f32| ((v * s).floor().max(0.0) as u32).saturating_sub(margin);
        let hi = |v: f32| ((v * s).ceil().max(0.0) as u32 + margin).min(size);
        let rect = Self {
            min: [lo(min[0]), lo(min[1])],
            max: [hi(max[0]), hi(max[1])],
        };
        if rect.is_empty() { Self::EMPTY } else { rect }
    }

    pub(crate) fn grow(self, margin: u32, size: u32) -> Self {
        if self.is_empty() {
            return self;
        }
        Self {
            min: self.min.map(|v| v.saturating_sub(margin)),
            max: self.max.map(|v| (v + margin).min(size)),
        }
    }

    /// The rectangle on mip `level`, rounded outwards.
    pub(crate) fn at_level(self, level: u32, size: u32) -> Self {
        let level_size = (size >> level).max(1);
        Self {
            min: self.min.map(|v| v >> level),
            max: self
                .max
                .map(|v| ((v + (1 << level) - 1) >> level).min(level_size)),
        }
    }

    /// Whether the rectangles overlap or are less than `gap` texels apart.
    pub(crate) fn near(&self, other: &Self, gap: u32) -> bool {
        self.min[0] <= other.max[0] + gap
            && other.min[0] <= self.max[0] + gap
            && self.min[1] <= other.max[1] + gap
            && other.min[1] <= self.max[1] + gap
    }

    #[cfg(test)]
    pub(crate) fn texels(&self) -> u64 {
        if self.is_empty() {
            0
        } else {
            (self.max[0] - self.min[0]) as u64 * (self.max[1] - self.min[1]) as u64
        }
    }

    pub(crate) fn scissor(&self, pass: &mut wgpu::RenderPass) {
        pass.set_scissor_rect(
            self.min[0],
            self.min[1],
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
        );
    }
}

/// Texels that changed, as a few rectangles: a single bounding box would span most of the
/// texture whenever the brush touches UV islands that lie far apart.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Region {
    pub rects: Vec<TexelRect>,
}

impl Region {
    /// Rectangles closer than this are merged.
    const MERGE_GAP: u32 = 16;
    /// More rectangles than this collapse into their bounding box.
    const MAX_RECTS: usize = 32;

    pub(crate) fn full(size: u32) -> Self {
        Self {
            rects: vec![TexelRect::full(size)],
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    pub(crate) fn add(&mut self, rect: TexelRect) {
        if rect.is_empty() {
            return;
        }
        let mut rect = rect;
        // Merging grows the rectangle, which can bring it near others.
        while let Some(i) = self
            .rects
            .iter()
            .position(|r| r.near(&rect, Self::MERGE_GAP))
        {
            rect = rect.union(self.rects.swap_remove(i));
        }
        self.rects.push(rect);
        if self.rects.len() > Self::MAX_RECTS {
            self.rects = vec![self.bounds()];
        }
    }

    pub(crate) fn extend(&mut self, other: &Self) {
        for &rect in &other.rects {
            self.add(rect);
        }
    }

    pub(crate) fn grown(&self, margin: u32, size: u32) -> Self {
        let mut grown = Self::default();
        for rect in &self.rects {
            grown.add(rect.grow(margin, size));
        }
        grown
    }

    pub(crate) fn bounds(&self) -> TexelRect {
        self.rects.iter().fold(TexelRect::EMPTY, |a, &b| a.union(b))
    }

    #[cfg(test)]
    pub(crate) fn texels(&self) -> u64 {
        self.rects.iter().map(TexelRect::texels).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_keep_distant_areas_apart() {
        let rect = |x: u32, y: u32, w: u32| TexelRect {
            min: [x, y],
            max: [x + w, y + w],
        };
        let mut region = Region::default();
        region.add(rect(0, 0, 10));
        region.add(rect(1000, 1000, 10));
        assert_eq!(region.rects.len(), 2);
        assert_eq!(region.texels(), 200);
        // Close rectangles merge, and the merge can reach a third one.
        region.add(rect(20, 0, 10));
        assert_eq!(region.rects.len(), 2);
        region.add(rect(40, 0, 10));
        assert_eq!(region.rects.len(), 2);
        assert!(region.rects.contains(&TexelRect {
            min: [0, 0],
            max: [50, 10]
        }));
        let grown = region.grown(5, 1024);
        assert_eq!(
            grown.bounds(),
            TexelRect {
                min: [0, 0],
                max: [1015, 1015]
            }
        );
        let mut many = Region::default();
        for i in 0..100 {
            many.add(rect(i * 100, 0, 1));
        }
        assert!(
            many.rects.len() <= Region::MAX_RECTS,
            "collapses to bounding boxes"
        );
        assert_eq!(
            many.bounds(),
            TexelRect {
                min: [0, 0],
                max: [9901, 1]
            }
        );
    }

    #[test]
    fn texel_rects() {
        let rect = TexelRect::from_uv([0.25, 0.5], [0.5, 0.75], 16, 1);
        assert_eq!(
            rect,
            TexelRect {
                min: [3, 7],
                max: [9, 13]
            }
        );
        assert_eq!(
            rect.at_level(1, 16),
            TexelRect {
                min: [1, 3],
                max: [5, 7]
            }
        );
        assert_eq!(
            rect.at_level(4, 16),
            TexelRect {
                min: [0, 0],
                max: [1, 1]
            }
        );
        assert!(TexelRect::from_uv([2.0, 2.0], [3.0, 3.0], 16, 0).is_empty());
        assert_eq!(TexelRect::EMPTY.union(rect), rect);
        assert_eq!(rect.grow(100, 16), TexelRect::full(16));
    }
}
