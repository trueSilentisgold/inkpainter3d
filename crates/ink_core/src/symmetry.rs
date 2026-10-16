//! Symmetry (Painter's L): a stroke painted once lands mirrored or turned around the model
//! too. A stroke keeps the copies as transforms ([`crate::Stroke::symmetry`]), so it paints
//! the same when it is painted again, whatever the settings are by then.

use crate::{Dab, StrokeCamera};
use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SymmetryMode {
    /// Reflected across the planes of the chosen axes.
    #[default]
    Mirror,
    /// Turned around an axis, in equal steps.
    Radial,
}

impl SymmetryMode {
    pub const ALL: [Self; 2] = [Self::Mirror, Self::Radial];

    pub fn label(self) -> &'static str {
        match self {
            Self::Mirror => "Mirror",
            Self::Radial => "Radial",
        }
    }
}

/// The symmetry settings of the brush tools.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Symmetry {
    /// Toggled with L.
    pub enabled: bool,
    pub mode: SymmetryMode,
    /// Mirror: the axes whose planes reflect; X mirrors left and right. Several together
    /// give every combination (X and Z: four copies of a stroke).
    pub mirror: [bool; 3],
    /// Radial: the axis the copies turn around (0 X, 1 Y, 2 Z).
    pub axis: usize,
    /// Radial: copies around the axis, the stroke itself included.
    pub count: u32,
    /// The point the planes and the axis pass through, in world space.
    pub center: Vec3,
}

impl Default for Symmetry {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: SymmetryMode::Mirror,
            mirror: [true, false, false],
            axis: 1,
            count: 4,
            center: Vec3::ZERO,
        }
    }
}

impl Symmetry {
    /// Most radial copies; each one paints the stroke again from a camera of its own. As
    /// many as three mirror planes give, so the renderer's depth maps of a stroke's cameras
    /// all stay cached while it is painted.
    pub const MAX_COUNT: u32 = 8;

    /// The transforms that make the copies of a stroke, the stroke itself left out: none if
    /// symmetry is off.
    pub fn copies(&self) -> Vec<Mat4> {
        if !self.enabled {
            return Vec::new();
        }
        let around = |m: Mat4| {
            Mat4::from_translation(self.center) * m * Mat4::from_translation(-self.center)
        };
        match self.mode {
            SymmetryMode::Mirror => {
                let axes: Vec<usize> = (0..3).filter(|&a| self.mirror[a]).collect();
                // Every non-empty combination of the planes.
                (1u32..1 << axes.len())
                    .map(|bits| {
                        let mut scale = Vec3::ONE;
                        for (i, &axis) in axes.iter().enumerate() {
                            if bits & (1 << i) != 0 {
                                scale[axis] = -1.0;
                            }
                        }
                        around(Mat4::from_scale(scale))
                    })
                    .collect()
            }
            SymmetryMode::Radial => {
                let count = self.count.clamp(1, Self::MAX_COUNT);
                let mut axis = Vec3::ZERO;
                axis[self.axis.min(2)] = 1.0;
                (1..count)
                    .map(|k| {
                        let angle = std::f32::consts::TAU * k as f32 / count as f32;
                        around(Mat4::from_axis_angle(axis, angle))
                    })
                    .collect()
            }
        }
    }
}

impl Dab {
    /// The dab moved by a symmetry transform (a mirror or a rotation).
    pub fn transformed(&self, m: &Mat4) -> Self {
        let turn = |v: Vec3| m.transform_vector3(v).normalize_or(v);
        Self {
            position: m.transform_point3(self.position),
            normal: turn(self.normal),
            up: turn(self.up),
            view: turn(self.view),
            ..*self
        }
    }
}

impl StrokeCamera {
    /// The camera that sees a symmetry copy of the stroke as this one sees the stroke: what
    /// hides the copy from it is what hides the stroke from this one, on a symmetric model.
    pub fn transformed(&self, m: &Mat4) -> Self {
        Self {
            view_proj: self.view_proj * m.inverse(),
            eye: m.transform_point3(self.eye),
            forward: m.transform_vector3(self.forward).normalize_or(self.forward),
            ..*self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> Symmetry {
        Symmetry {
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn mirrors_combine_and_radial_turns() {
        assert!(Symmetry::default().copies().is_empty(), "off by default");
        assert_eq!(on().copies().len(), 1);
        let xz = Symmetry {
            mirror: [true, false, true],
            ..on()
        };
        assert_eq!(xz.copies().len(), 3);
        let radial = Symmetry {
            mode: SymmetryMode::Radial,
            count: 6,
            ..on()
        };
        assert_eq!(radial.copies().len(), 5);
    }

    #[test]
    fn a_mirror_reflects_across_its_plane_through_the_center() {
        let s = Symmetry {
            center: Vec3::new(1.0, 0.0, 0.0),
            ..on()
        };
        let m = s.copies()[0];
        let p = m.transform_point3(Vec3::new(3.0, 2.0, -1.0));
        assert!((p - Vec3::new(-1.0, 2.0, -1.0)).length() < 1e-5);
    }

    #[test]
    fn a_copy_seen_by_the_copied_camera_lands_where_the_original_did() {
        // A perspective projection (right-handed, looking down -z) from a camera at `eye`.
        let eye = Vec3::new(2.0, 1.0, 5.0);
        let (f, aspect, near, far) = (1.8, 1.5, 0.1, 100.0);
        let projection = Mat4::from_cols_array(&[
            f / aspect,
            0.0,
            0.0,
            0.0,
            0.0,
            f,
            0.0,
            0.0,
            0.0,
            0.0,
            far / (near - far),
            -1.0,
            0.0,
            0.0,
            near * far / (near - far),
            0.0,
        ]);
        let camera = StrokeCamera {
            view_proj: projection * Mat4::from_translation(-eye),
            eye,
            forward: Vec3::NEG_Z,
            aspect: 1.5,
            orthographic: false,
            height: 1.0,
        };
        let radial = Symmetry {
            mode: SymmetryMode::Radial,
            count: 3,
            ..on()
        };
        for m in on().copies().into_iter().chain(radial.copies()) {
            let p = Vec3::new(0.3, 0.2, 0.4);
            let copy = camera.transformed(&m);
            let a = camera.view_proj.project_point3(p);
            let b = copy.view_proj.project_point3(m.transform_point3(p));
            assert!((a - b).length() < 1e-4);
        }
    }
}
