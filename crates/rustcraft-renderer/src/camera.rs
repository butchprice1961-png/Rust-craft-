//! First-person camera using Minecraft's angle conventions, and frustum
//! culling. Rendering is camera-relative: the camera sits at the origin and
//! geometry is offset by `section - camera` in f64 first, so precision does
//! not degrade far from the world origin.

use glam::{DVec3, Mat4, Vec3, Vec4};

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// Eye position.
    pub pos: DVec3,
    /// Degrees; 0 faces south (+Z), 90 faces west (-X).
    pub yaw: f32,
    /// Degrees; positive looks down.
    pub pitch: f32,
    pub fov_y: f32,
    pub aspect: f32,
    pub far: f32,
}

impl Camera {
    pub fn forward(&self) -> Vec3 {
        let (yaw, pitch) = (self.yaw.to_radians(), self.pitch.to_radians());
        Vec3::new(-yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos())
    }

    pub fn view_proj(&self) -> Mat4 {
        // wgpu uses the DirectX clip-space convention (depth 0–1, Y up).
        let view = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, self.forward(), Vec3::Y);
        let proj = glam::camera::rh::proj::directx::perspective(
            self.fov_y.to_radians(),
            self.aspect.max(0.01),
            0.05,
            self.far,
        );
        proj * view
    }
}

/// Six planes; a point p is inside when dot(plane.xyz, p) + plane.w >= 0.
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    pub fn from_matrix(m: Mat4) -> Self {
        let r = [m.row(0), m.row(1), m.row(2), m.row(3)];
        let planes = [r[3] + r[0], r[3] - r[0], r[3] + r[1], r[3] - r[1], r[2], r[3] - r[2]];
        Self { planes: planes.map(|p| p / p.truncate().length()) }
    }

    /// Conservative box test (camera-relative coordinates).
    pub fn contains_box(&self, min: Vec3, max: Vec3) -> bool {
        self.planes.iter().all(|p| {
            let n = p.truncate();
            // The box corner furthest along the plane normal.
            let v = Vec3::select(n.cmpge(Vec3::ZERO), max, min);
            n.dot(v) + p.w >= 0.0
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(yaw: f32, pitch: f32) -> Camera {
        Camera { pos: DVec3::ZERO, yaw, pitch, fov_y: 70.0, aspect: 16.0 / 9.0, far: 256.0 }
    }

    #[test]
    fn minecraft_angle_conventions() {
        assert!((cam(0.0, 0.0).forward() - Vec3::Z).length() < 1e-5);
        assert!((cam(90.0, 0.0).forward() - Vec3::NEG_X).length() < 1e-5);
        assert!((cam(0.0, 90.0).forward() - Vec3::NEG_Y).length() < 1e-5);
    }

    #[test]
    fn frustum_keeps_what_is_ahead() {
        let f = Frustum::from_matrix(cam(0.0, 0.0).view_proj());
        assert!(f.contains_box(Vec3::new(-1.0, -1.0, 10.0), Vec3::new(1.0, 1.0, 12.0)));
        assert!(!f.contains_box(Vec3::new(-1.0, -1.0, -12.0), Vec3::new(1.0, 1.0, -10.0)));
        assert!(!f.contains_box(Vec3::new(-1.0, -1.0, 300.0), Vec3::new(1.0, 1.0, 302.0)));
        // A big box surrounding the camera is visible.
        assert!(f.contains_box(Vec3::splat(-8.0), Vec3::splat(8.0)));
    }
}
