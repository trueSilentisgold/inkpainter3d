use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3};
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
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.35,
            distance: 4.0,
            fov_y: 40f32.to_radians(),
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
        let near = (self.distance * 0.01).max(1e-4);
        let far = self.distance * 100.0 + 100.0;
        // wgpu uses the DirectX convention: NDC depth in [0, 1], Y up.
        directx::perspective(self.fov_y, aspect.max(1e-4), near, far)
    }

    /// Rotates by a mouse delta in pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        const SPEED: f32 = 0.008;
        self.yaw -= dx * SPEED;
        self.pitch = (self.pitch + dy * SPEED).clamp(-Self::PITCH_LIMIT, Self::PITCH_LIMIT);
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
    fn pitch_is_clamped() {
        let mut cam = OrbitCamera::default();
        cam.orbit(0.0, 1e6);
        assert!(cam.pitch <= OrbitCamera::PITCH_LIMIT);
        assert!(cam.view().is_finite());
    }
}
