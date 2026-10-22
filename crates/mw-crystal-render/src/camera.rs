use glam::{Mat4, Vec3};

/// View and projection matrices for one frame.
#[derive(Clone, Copy, Debug)]
pub struct CameraMatrices {
    pub view: Mat4,
    pub proj: Mat4,
}

/// Orthographic camera orbiting a target point.
///
/// The crystal viewport defaults to an orthographic projection, so zoom is
/// the half-height of the visible region rather than a distance.
#[derive(Clone, Copy, Debug)]
pub struct OrbitCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    half_height: f32,
    scene_radius: f32,
}

const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

impl OrbitCamera {
    /// Frames a scene of the given bounding radius around the origin.
    pub fn framing(scene_radius: f32) -> Self {
        let scene_radius = scene_radius.max(1.0);
        Self {
            target: Vec3::ZERO,
            yaw: 0.6,
            pitch: 0.35,
            half_height: scene_radius * 1.1,
            scene_radius,
        }
    }

    /// Adapts depth range and zoom limits to a scene of a new size, keeping
    /// the current angle and zoom.
    pub fn set_scene_radius(&mut self, scene_radius: f32) {
        self.scene_radius = scene_radius.max(1.0);
    }

    /// Rotates by the given angles in radians.
    pub fn rotate(&mut self, delta_yaw: f32, delta_pitch: f32) {
        self.yaw = (self.yaw + delta_yaw) % std::f32::consts::TAU;
        self.pitch = (self.pitch + delta_pitch).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Zooms in for factors above 1 and out for factors below 1.
    pub fn zoom(&mut self, factor: f32) {
        let min = self.scene_radius * 0.02;
        let max = self.scene_radius * 10.0;
        self.half_height = (self.half_height / factor.max(f32::EPSILON)).clamp(min, max);
    }

    /// Visible world-space height per screen pixel, for converting drags.
    pub fn world_per_pixel(&self, viewport_height_px: f32) -> f32 {
        2.0 * self.half_height / viewport_height_px.max(1.0)
    }

    pub fn matrices(&self, aspect: f32) -> CameraMatrices {
        let direction = Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        );
        let distance = self.scene_radius * 2.0;
        let eye = self.target + direction * distance;
        let view = Mat4::look_at_rh(eye, self.target, Vec3::Y);
        let half_width = self.half_height * aspect;
        let proj = Mat4::orthographic_rh(
            -half_width,
            half_width,
            -self.half_height,
            self.half_height,
            0.01,
            distance + self.scene_radius * 2.0,
        );
        CameraMatrices { view, proj }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_projects_to_center_inside_depth_range() {
        let camera = OrbitCamera::framing(10.0);
        let m = camera.matrices(1.5);
        let clip = m.proj * m.view * Vec3::ZERO.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() < 1e-5 && ndc.y.abs() < 1e-5);
        assert!((0.0..=1.0).contains(&ndc.z), "depth {}", ndc.z);
    }

    #[test]
    fn whole_scene_stays_inside_depth_range_at_any_angle() {
        let mut camera = OrbitCamera::framing(10.0);
        for _ in 0..16 {
            camera.rotate(0.7, 0.3);
            let m = camera.matrices(1.0);
            for corner in [Vec3::splat(10.0), Vec3::splat(-10.0)] {
                let clip = m.proj * m.view * corner.extend(1.0);
                let z = clip.z / clip.w;
                assert!((0.0..=1.0).contains(&z), "depth {z}");
            }
        }
    }

    #[test]
    fn zoom_is_clamped() {
        let mut camera = OrbitCamera::framing(10.0);
        camera.zoom(1e9);
        assert!(camera.world_per_pixel(100.0) > 0.0);
        camera.zoom(1e-9);
        assert!(camera.world_per_pixel(100.0).is_finite());
    }
}
