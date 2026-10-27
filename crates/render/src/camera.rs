//! Orbit camera: the single camera model every CosmicDMS canvas uses.
//!
//! The camera orbits `target` at `distance`. `yaw` is measured around +Z from +X,
//! `pitch` is the elevation above the XY plane. Both are radians. The view basis is
//! built explicitly so straight top and bottom views have no gimbal singularity.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use CosmicDMS_geom::{Aabb, Mat4, Vec2, Vec3};

pub const MIN_DISTANCE: f32 = 0.05;
pub const MAX_DISTANCE: f32 = 1.0e6;
const DEFAULT_FOV_Y: f32 = 0.6;
/// atan(1/sqrt(2)): the elevation of a true isometric view.
const ISO_PITCH: f32 = 0.615_479_7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    Perspective,
    Orthographic,
}

impl Projection {
    pub fn id(self) -> &'static str {
        match self {
            Self::Perspective => "perspective",
            Self::Orthographic => "orthographic",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "perspective" => Some(Self::Perspective),
            "orthographic" => Some(Self::Orthographic),
            _ => None,
        }
    }
}

/// Standard orientations, reachable from the view cube, the ribbon and commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewPreset {
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
    IsoFrontRight,
    IsoFrontLeft,
    IsoBackRight,
    IsoBackLeft,
}

impl ViewPreset {
    pub const ALL: [Self; 10] = [
        Self::Top,
        Self::Bottom,
        Self::Front,
        Self::Back,
        Self::Left,
        Self::Right,
        Self::IsoFrontRight,
        Self::IsoFrontLeft,
        Self::IsoBackRight,
        Self::IsoBackLeft,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Front => "front",
            Self::Back => "back",
            Self::Left => "left",
            Self::Right => "right",
            Self::IsoFrontRight => "iso-front-right",
            Self::IsoFrontLeft => "iso-front-left",
            Self::IsoBackRight => "iso-back-right",
            Self::IsoBackLeft => "iso-back-left",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        if id == "iso" {
            return Some(Self::IsoFrontRight);
        }
        Self::ALL.into_iter().find(|p| p.id() == id)
    }

    /// (yaw, pitch) in radians.
    pub fn angles(self) -> (f32, f32) {
        match self {
            Self::Top => (-FRAC_PI_2, FRAC_PI_2),
            Self::Bottom => (-FRAC_PI_2, -FRAC_PI_2),
            Self::Front => (-FRAC_PI_2, 0.0),
            Self::Back => (FRAC_PI_2, 0.0),
            Self::Left => (PI, 0.0),
            Self::Right => (0.0, 0.0),
            Self::IsoFrontRight => (-FRAC_PI_4, ISO_PITCH),
            Self::IsoFrontLeft => (-3.0 * FRAC_PI_4, ISO_PITCH),
            Self::IsoBackRight => (FRAC_PI_4, ISO_PITCH),
            Self::IsoBackLeft => (3.0 * FRAC_PI_4, ISO_PITCH),
        }
    }
}

/// Orthonormal view basis: `right` and `up` span the screen, `forward` looks into it.
#[derive(Clone, Copy, Debug)]
pub struct ViewBasis {
    pub right: Vec3,
    pub up: Vec3,
    pub forward: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
    pub projection: Projection,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        let (yaw, pitch) = ViewPreset::IsoFrontRight.angles();
        Self {
            target: Vec3::ZERO,
            distance: 250.0,
            yaw,
            pitch,
            fov_y: DEFAULT_FOV_Y,
            projection: Projection::Orthographic,
        }
    }
}

impl OrbitCamera {
    /// Unit vector from `target` towards the eye.
    pub fn eye_dir(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(cp * cy, cp * sy, sp)
    }

    pub fn eye(&self) -> Vec3 {
        self.target + self.eye_dir() * self.distance
    }

    pub fn basis(&self) -> ViewBasis {
        let (sy, cy) = self.yaw.sin_cos();
        let forward = -self.eye_dir();
        let right = Vec3::new(-sy, cy, 0.0);
        let up = right.cross(forward);
        ViewBasis { right, up, forward }
    }

    pub fn view_matrix(&self) -> Mat4 {
        let b = self.basis();
        Mat4::look_to_rh(self.eye(), b.forward, b.up)
    }

    /// Half height of the view at the target plane, in millimetres.
    pub fn half_height_at_target(&self) -> f32 {
        self.distance * (self.fov_y * 0.5).tan()
    }

    /// Projection with wgpu depth range (0..1). `scene_radius` keeps the clip planes tight.
    pub fn projection_matrix(&self, aspect: f32, scene_radius: f32) -> Mat4 {
        let aspect = if aspect.is_finite() && aspect > 1.0e-3 { aspect } else { 1.0 };
        let reach = (scene_radius.max(1.0) * 3.0).max(self.distance * 0.5);
        match self.projection {
            Projection::Perspective => {
                let near = (self.distance - reach).max(self.distance * 0.002).max(0.01);
                let far = (self.distance + reach).max(near + 1.0);
                Mat4::perspective_rh(self.fov_y, aspect, near, far)
            }
            Projection::Orthographic => {
                let h = self.half_height_at_target();
                let w = h * aspect;
                Mat4::orthographic_rh(-w, w, -h, h, self.distance - reach, self.distance + reach)
            }
        }
    }

    pub fn view_proj(&self, aspect: f32, scene_radius: f32) -> Mat4 {
        self.projection_matrix(aspect, scene_radius) * self.view_matrix()
    }

    pub fn set_preset(&mut self, preset: ViewPreset) {
        let (yaw, pitch) = preset.angles();
        self.yaw = yaw;
        self.pitch = pitch;
    }

    /// Frames `bounds` in a viewport of the given aspect, keeping the orientation.
    pub fn fit(&mut self, bounds: &Aabb, aspect: f32) {
        if bounds.is_empty() {
            return;
        }
        let radius = bounds.radius().max(1.0);
        let aspect = if aspect.is_finite() && aspect > 1.0e-3 { aspect } else { 1.0 };
        let half_v = self.fov_y * 0.5;
        let half_h = (half_v.tan() * aspect).atan();
        let half_fov = half_v.min(half_h);
        self.target = bounds.center();
        self.distance = (radius * 1.15 / half_fov.sin()).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Orbit by a screen-space drag in pixels.
    pub fn orbit(&mut self, delta: Vec2) {
        if !delta.is_finite() {
            return;
        }
        self.yaw = wrap_angle(self.yaw - delta.x * 0.008);
        self.pitch = (self.pitch + delta.y * 0.008).clamp(-FRAC_PI_2, FRAC_PI_2);
    }

    /// Pan so the point under the cursor follows a drag of `delta` pixels.
    pub fn pan(&mut self, delta: Vec2, viewport_height_px: f32) {
        if !delta.is_finite() || viewport_height_px.is_nan() || viewport_height_px <= 1.0 {
            return;
        }
        let mm_per_px = 2.0 * self.half_height_at_target() / viewport_height_px;
        let b = self.basis();
        self.target += (-b.right * delta.x + b.up * delta.y) * mm_per_px;
    }

    /// Zoom by `factor` (< 1 zooms in) about a cursor at `ndc` (-1..1, y up).
    pub fn zoom_at(&mut self, factor: f32, ndc: Vec2, aspect: f32) {
        if !factor.is_finite() || factor <= 0.0 || !ndc.is_finite() {
            return;
        }
        let new_distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
        let applied = new_distance / self.distance;
        let h = self.half_height_at_target();
        let aspect = if aspect.is_finite() && aspect > 1.0e-3 { aspect } else { 1.0 };
        let b = self.basis();
        let anchor = self.target + b.right * (ndc.x * h * aspect) + b.up * (ndc.y * h);
        self.target = anchor + (self.target - anchor) * applied;
        self.distance = new_distance;
    }

    /// Interpolates towards `to`, taking the short way round in yaw.
    pub fn lerp(&self, to: &Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let dyaw = wrap_angle(to.yaw - self.yaw);
        let distance = (self.distance.ln() + (to.distance.ln() - self.distance.ln()) * t).exp();
        Self {
            target: self.target.lerp(to.target, t),
            distance: if distance.is_finite() { distance } else { to.distance },
            yaw: wrap_angle(self.yaw + dyaw * t),
            pitch: self.pitch + (to.pitch - self.pitch) * t,
            fov_y: to.fov_y,
            projection: to.projection,
        }
    }

    /// True when `other` is visually indistinguishable from `self`.
    pub fn approx_eq(&self, other: &Self) -> bool {
        let scale = self.distance.max(1.0);
        wrap_angle(self.yaw - other.yaw).abs() < 1.0e-4
            && (self.pitch - other.pitch).abs() < 1.0e-4
            && (self.distance - other.distance).abs() < scale * 1.0e-4
            && self.target.distance(other.target) < scale * 1.0e-4
            && self.projection == other.projection
    }

    /// Replaces any non-finite field with the default so a bad value never reaches the GPU.
    pub fn sanitized(mut self) -> Self {
        let d = Self::default();
        if !self.target.is_finite() {
            self.target = d.target;
        }
        if !self.distance.is_finite() {
            self.distance = d.distance;
        }
        self.distance = self.distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
        if !self.yaw.is_finite() {
            self.yaw = d.yaw;
        }
        if !self.pitch.is_finite() {
            self.pitch = d.pitch;
        }
        self.pitch = self.pitch.clamp(-FRAC_PI_2, FRAC_PI_2);
        if !(self.fov_y.is_finite() && self.fov_y > 0.05 && self.fov_y < 3.0) {
            self.fov_y = d.fov_y;
        }
        self
    }
}

/// Wraps an angle to (-PI, PI].
pub fn wrap_angle(a: f32) -> f32 {
    if !a.is_finite() {
        return 0.0;
    }
    let mut w = (a + PI).rem_euclid(TAU) - PI;
    if w <= -PI {
        w += TAU;
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1.0e-5
    }

    #[test]
    fn presets_look_along_the_expected_axes() {
        let mut cam = OrbitCamera::default();
        cam.set_preset(ViewPreset::Front);
        assert!(close(cam.basis().forward, Vec3::Y));
        assert!(close(cam.basis().right, Vec3::X));
        assert!(close(cam.basis().up, Vec3::Z));

        cam.set_preset(ViewPreset::Top);
        assert!(close(cam.basis().forward, -Vec3::Z));
        assert!(close(cam.basis().right, Vec3::X));
        assert!(close(cam.basis().up, Vec3::Y));

        cam.set_preset(ViewPreset::Right);
        assert!(close(cam.basis().forward, -Vec3::X));
        assert!(close(cam.basis().up, Vec3::Z));
    }

    #[test]
    fn preset_ids_round_trip() {
        for p in ViewPreset::ALL {
            assert_eq!(ViewPreset::from_id(p.id()), Some(p));
        }
        assert_eq!(ViewPreset::from_id("iso"), Some(ViewPreset::IsoFrontRight));
        assert_eq!(ViewPreset::from_id("sideways"), None);
    }

    #[test]
    fn view_proj_is_finite_for_both_projections_and_extreme_pitch() {
        for projection in [Projection::Perspective, Projection::Orthographic] {
            for preset in ViewPreset::ALL {
                let mut cam = OrbitCamera { projection, ..Default::default() };
                cam.set_preset(preset);
                let m = cam.view_proj(1.6, 80.0);
                assert!(m.is_finite(), "{preset:?} {projection:?}");
            }
        }
    }

    #[test]
    fn target_projects_to_screen_centre() {
        let cam = OrbitCamera { target: Vec3::new(10.0, -5.0, 3.0), ..Default::default() };
        let clip = cam.view_proj(1.5, 50.0) * cam.target.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() < 1.0e-4 && ndc.y.abs() < 1.0e-4);
        assert!((0.0..=1.0).contains(&ndc.z));
    }

    #[test]
    fn fit_contains_the_bounds() {
        let bounds = Aabb::new(Vec3::new(-50.0, -30.0, 0.0), Vec3::new(50.0, 30.0, 35.0));
        let mut cam = OrbitCamera::default();
        cam.fit(&bounds, 1.6);
        let m = cam.view_proj(1.6, bounds.radius());
        for x in [bounds.min.x, bounds.max.x] {
            for y in [bounds.min.y, bounds.max.y] {
                for z in [bounds.min.z, bounds.max.z] {
                    let clip = m * Vec3::new(x, y, z).extend(1.0);
                    let ndc = clip.truncate() / clip.w;
                    assert!(ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0, "{ndc:?}");
                    assert!((0.0..=1.0).contains(&ndc.z), "{ndc:?}");
                }
            }
        }
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut cam = OrbitCamera::default();
        let ndc = Vec2::new(0.5, -0.25);
        let aspect = 1.5;
        let before = cam.view_proj(aspect, 100.0).inverse();
        let world = before.project_point3(Vec3::new(ndc.x, ndc.y, 0.5));
        let screen_before = cam.view_proj(aspect, 100.0).project_point3(world);
        cam.zoom_at(0.5, ndc, aspect);
        let screen_after = cam.view_proj(aspect, 100.0).project_point3(world);
        assert!((screen_before.truncate() - screen_after.truncate()).length() < 1.0e-3);
    }

    #[test]
    fn hostile_input_never_produces_nan() {
        let mut cam = OrbitCamera::default();
        cam.orbit(Vec2::new(f32::NAN, 1.0));
        cam.pan(Vec2::new(f32::INFINITY, 0.0), 600.0);
        cam.pan(Vec2::ONE, 0.0);
        cam.zoom_at(f32::NAN, Vec2::ZERO, 1.0);
        cam.zoom_at(-1.0, Vec2::ZERO, 1.0);
        cam.zoom_at(1.0e30, Vec2::ZERO, f32::NAN);
        assert!(cam.view_proj(f32::NAN, f32::NAN).is_finite());
        let bad = OrbitCamera {
            target: Vec3::NAN,
            distance: f32::NAN,
            yaw: f32::INFINITY,
            pitch: f32::NAN,
            fov_y: 0.0,
            projection: Projection::Perspective,
        }
        .sanitized();
        assert!(bad.view_proj(1.0, 10.0).is_finite());
    }

    #[test]
    fn lerp_takes_the_short_way_round() {
        let a = OrbitCamera { yaw: 3.0, ..Default::default() };
        let b = OrbitCamera { yaw: -3.0, ..Default::default() };
        let mid = a.lerp(&b, 0.5);
        assert!(mid.yaw.abs() > 3.0, "{}", mid.yaw);
        assert!(a.lerp(&b, 1.0).approx_eq(&b));
    }
}
