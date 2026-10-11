use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec2, Vec3, Vec4Swizzles};
use ink_core::Aabb;

/// Turntable camera orbiting around a target point (Y up).
#[derive(Clone, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    /// Rotation around the Y axis in radians.
    pub yaw: f32,
    /// Elevation in radians, clamped just short of the poles.
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
    /// Orthographic projection; `distance` then sets the visible height.
    pub orthographic: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.35,
            distance: 4.0,
            fov_y: 40f32.to_radians(),
            orthographic: false,
        }
    }
}

impl OrbitCamera {
    const PITCH_LIMIT: f32 = 1.55;

    pub fn eye(&self) -> Vec3 {
        let dir = Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        );
        self.target + dir * self.distance
    }

    pub fn view(&self) -> Mat4 {
        view::look_at_mat4(self.eye(), self.target, Vec3::Y)
    }

    pub fn projection(&self, aspect: f32) -> Mat4 {
        let aspect = aspect.max(1e-4);
        let far = self.distance * 100.0 + 100.0;
        // wgpu uses the DirectX convention: NDC depth in [0, 1], Y up.
        if self.orthographic {
            let h = self.distance * (self.fov_y * 0.5).tan();
            let w = h * aspect;
            // The near plane lies behind the eye so nothing gets clipped when zoomed in.
            directx::orthographic(-w, w, -h, h, -far, far)
        } else {
            let near = (self.distance * 0.01).max(1e-4);
            directx::perspective(self.fov_y, aspect, near, far)
        }
    }

    /// Unit vector from the eye towards the target.
    pub fn forward(&self) -> Vec3 {
        (self.target - self.eye()).normalize()
    }

    /// World space ray through a point of the viewport, given in pixels from the top left.
    /// Returns origin and unit direction.
    pub fn ray(&self, pixel: Vec2, viewport: Vec2) -> (Vec3, Vec3) {
        let ndc = Vec2::new(
            pixel.x / viewport.x * 2.0 - 1.0,
            1.0 - pixel.y / viewport.y * 2.0,
        );
        let inv = (self.projection(viewport.x / viewport.y) * self.view()).inverse();
        let unproject = |z: f32| {
            let p = inv * ndc.extend(z).extend(1.0);
            p.xyz() / p.w
        };
        let (near, far) = (unproject(0.0), unproject(1.0));
        (near, (far - near).normalize())
    }

    /// Rotates by a mouse delta in pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        const SPEED: f32 = 0.008;
        self.yaw -= dx * SPEED;
        self.pitch = (self.pitch + dy * SPEED).clamp(-Self::PITCH_LIMIT, Self::PITCH_LIMIT);
    }

    /// Sets yaw and pitch to the nearest multiple of 90 degrees of the given angles,
    /// for snapping to front, side, back, top and bottom views.
    pub fn set_snapped(&mut self, yaw: f32, pitch: f32) {
        let step = std::f32::consts::FRAC_PI_2;
        self.yaw = (yaw / step).round() * step;
        self.pitch = ((pitch / step).round() * step).clamp(-Self::PITCH_LIMIT, Self::PITCH_LIMIT);
    }

    /// Moves the target in the view plane so the point under the cursor follows the mouse.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let view = self.view();
        let right = view.row(0).truncate();
        let up = view.row(1).truncate();
        let world_per_pixel =
            2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_height.max(1.0);
        self.target += (-right * dx + up * dy) * world_per_pixel;
    }

    /// Zooms by scroll steps; positive values move closer.
    pub fn zoom(&mut self, steps: f32) {
        self.distance = (self.distance * 0.9f32.powf(steps)).clamp(1e-3, 1e5);
    }

    /// Centers the camera on the bounds so they fill the view.
    pub fn frame(&mut self, bounds: &Aabb) {
        self.target = bounds.center();
        let radius = bounds.radius().max(1e-3);
        self.distance = radius / (self.fov_y * 0.5).sin() * 1.1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_keeps_bounds_in_front_of_camera() {
        let mut cam = OrbitCamera::default();
        let bounds = Aabb {
            min: Vec3::new(9.0, -1.0, -1.0),
            max: Vec3::new(11.0, 1.0, 1.0),
        };
        cam.frame(&bounds);
        assert_eq!(cam.target, Vec3::new(10.0, 0.0, 0.0));
        assert!(cam.eye().distance(cam.target) > bounds.radius());
    }

    #[test]
    fn ray_through_center_hits_target() {
        for orthographic in [false, true] {
            let cam = OrbitCamera {
                orthographic,
                ..Default::default()
            };
            let size = Vec2::new(800.0, 600.0);
            let (origin, dir) = cam.ray(size * 0.5, size);
            let to_target = cam.target - origin;
            let off_axis = to_target - dir * to_target.dot(dir);
            assert!(off_axis.length() < 1e-3, "orthographic: {orthographic}");
            assert!((dir - cam.forward()).length() < 1e-4);
        }
    }

    #[test]
    fn snapping_picks_axis_views() {
        let mut cam = OrbitCamera::default();
        cam.set_snapped(1.7, 0.3);
        assert!((cam.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(cam.pitch, 0.0);
        cam.set_snapped(0.0, 1.4);
        assert_eq!(cam.pitch, OrbitCamera::PITCH_LIMIT);
    }

    #[test]
    fn pitch_is_clamped() {
        let mut cam = OrbitCamera::default();
        cam.orbit(0.0, 1e6);
        assert!(cam.pitch <= OrbitCamera::PITCH_LIMIT);
        assert!(cam.view().is_finite());
    }
}
