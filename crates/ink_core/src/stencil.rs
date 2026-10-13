//! The stencil: an image laid over the view that the brush paints through.

use crate::AlphaImage;
use glam::Vec2;
use std::sync::Arc;

/// The stencil's image and where it lies in the view. Positions are in view units: the
/// view is 2 units high, y points up and the middle is (0, 0).
#[derive(Clone, Debug)]
pub struct Stencil {
    pub image: Option<Arc<AlphaImage>>,
    pub enabled: bool,
    pub center: Vec2,
    /// Height of the image in view units.
    pub size: f32,
    /// Counter-clockwise on screen, in degrees.
    pub rotation: f32,
    /// How visible the stencil is drawn over the view, 0 to 1. Does not affect painting.
    pub overlay: f32,
}

impl Default for Stencil {
    fn default() -> Self {
        Self {
            image: None,
            enabled: true,
            center: Vec2::ZERO,
            size: 1.0,
            rotation: 0.0,
            overlay: 0.5,
        }
    }
}

impl Stencil {
    /// The image, if the stencil is on and has one.
    pub fn active(&self) -> Option<&Arc<AlphaImage>> {
        self.image.as_ref().filter(|_| self.enabled)
    }

    /// Maps view coordinates to the image's `[-1, 1]` square (y up), as two rows
    /// `(a, b, c)`: `x' = a x + b y + c`.
    pub fn view_affine(&self) -> Option<[f32; 6]> {
        let image = self.image.as_ref()?;
        let aspect = image.width as f32 / image.height.max(1) as f32;
        let (hx, hy) = (
            self.size.max(1e-4) * aspect * 0.5,
            self.size.max(1e-4) * 0.5,
        );
        let (s, c) = self.rotation.to_radians().sin_cos();
        let (cx, cy) = (self.center.x, self.center.y);
        Some([
            c / hx,
            s / hx,
            -(c * cx + s * cy) / hx,
            -s / hy,
            c / hy,
            -(-s * cx + c * cy) / hy,
        ])
    }

    /// Moves the stencil by a pointer movement in view units.
    pub fn translate(&mut self, delta: Vec2) {
        self.center += delta;
    }

    /// Scales the stencil around its center.
    pub fn scale(&mut self, factor: f32) {
        self.size = (self.size * factor).clamp(0.01, 50.0);
    }
}

/// The stencil as a stroke keeps it: the image and how a point of the view (or, in the 2D
/// view, of the UV square) maps into it.
#[derive(Clone, Debug)]
pub struct StrokeStencil {
    pub image: Arc<AlphaImage>,
    /// Two rows `(a, b, c)`, see [`Stencil::view_affine`]; the input is `(ndc.x * aspect,
    /// ndc.y)` for surface strokes and the UV for strokes of the 2D view.
    pub affine: [f32; 6],
}

impl PartialEq for StrokeStencil {
    fn eq(&self, other: &Self) -> bool {
        self.image.id == other.image.id && self.affine == other.affine
    }
}

/// `outer` after `inner`: both map `(x, y)` with rows `(a, b, c)`.
pub fn compose(outer: [f32; 6], inner: [f32; 6]) -> [f32; 6] {
    let [oa, ob, oc, od, oe, of] = outer;
    let [ia, ib, ic, id, ie, iff] = inner;
    [
        oa * ia + ob * id,
        oa * ib + ob * ie,
        oa * ic + ob * iff + oc,
        od * ia + oe * id,
        od * ib + oe * ie,
        od * ic + oe * iff + of,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: [f32; 6], p: Vec2) -> Vec2 {
        Vec2::new(
            m[0] * p.x + m[1] * p.y + m[2],
            m[3] * p.x + m[4] * p.y + m[5],
        )
    }

    #[test]
    fn the_image_lies_around_its_center() {
        let image = Arc::new(AlphaImage::new("s".into(), 2, 1, vec![255, 255]));
        let mut stencil = Stencil {
            image: Some(image),
            center: Vec2::new(0.5, 0.25),
            size: 0.5,
            ..Stencil::default()
        };
        let m = stencil.view_affine().unwrap();
        assert!(apply(m, Vec2::new(0.5, 0.25)).length() < 1e-5);
        // A 2:1 image of height 0.5 is 1 wide: its right edge is 0.5 from the center.
        assert!((apply(m, Vec2::new(1.0, 0.25)) - Vec2::X).length() < 1e-5);
        assert!((apply(m, Vec2::new(0.5, 0.5)) - Vec2::Y).length() < 1e-5);
        // Turned a quarter counter-clockwise, the image's right edge points up.
        stencil.rotation = 90.0;
        let m = stencil.view_affine().unwrap();
        assert!((apply(m, Vec2::new(0.5, 0.75)) - Vec2::X).length() < 1e-5);
    }

    #[test]
    fn composing_applies_the_inner_map_first() {
        let outer = [2.0, 0.0, 1.0, 0.0, 2.0, 0.0];
        let inner = [1.0, 0.0, 3.0, 0.0, -1.0, 0.0];
        let p = Vec2::new(0.3, 0.7);
        let both = compose(outer, inner);
        assert!((apply(both, p) - apply(outer, apply(inner, p))).length() < 1e-6);
    }

    #[test]
    fn off_or_empty_stencils_are_inactive() {
        let mut stencil = Stencil::default();
        assert!(stencil.active().is_none() && stencil.view_affine().is_none());
        stencil.image = Some(Arc::new(AlphaImage::new("s".into(), 1, 1, vec![9])));
        assert!(stencil.active().is_some());
        stencil.enabled = false;
        assert!(stencil.active().is_none());
    }
}
