//! View cube geometry: which faces and corners face the viewer and where they land on screen.
//!
//! Coordinates are in "cube units" with the cube spanning -1..1 and screen Y pointing up;
//! the UI scales and flips them into its overlay rectangle.

use CosmicCAM_geom::{Vec2, Vec3};

use crate::camera::{OrbitCamera, ViewPreset};

#[derive(Clone, Copy, Debug)]
pub struct CubeFace {
    pub preset: ViewPreset,
    pub label: &'static str,
    pub corners: [Vec2; 4],
    /// How squarely the face points at the viewer, 0..1.
    pub facing: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct CubeCorner {
    pub preset: ViewPreset,
    pub at: Vec2,
}

#[derive(Clone, Copy, Debug)]
pub enum CubeHit {
    Face(ViewPreset),
    Corner(ViewPreset),
}

impl CubeHit {
    pub fn preset(self) -> ViewPreset {
        match self {
            Self::Face(p) | Self::Corner(p) => p,
        }
    }
}

const FACES: [(ViewPreset, &str, Vec3, Vec3, Vec3); 6] = [
    // preset, label, normal, in-face "right", in-face "up"
    (ViewPreset::Top, "TOP", Vec3::Z, Vec3::X, Vec3::Y),
    (ViewPreset::Bottom, "BOTTOM", Vec3::NEG_Z, Vec3::X, Vec3::NEG_Y),
    (ViewPreset::Front, "FRONT", Vec3::NEG_Y, Vec3::X, Vec3::Z),
    (ViewPreset::Back, "BACK", Vec3::Y, Vec3::NEG_X, Vec3::Z),
    (ViewPreset::Right, "RIGHT", Vec3::X, Vec3::Y, Vec3::Z),
    (ViewPreset::Left, "LEFT", Vec3::NEG_X, Vec3::NEG_Y, Vec3::Z),
];

const TOP_CORNERS: [(ViewPreset, Vec3); 4] = [
    (ViewPreset::IsoFrontRight, Vec3::new(1.0, -1.0, 1.0)),
    (ViewPreset::IsoFrontLeft, Vec3::new(-1.0, -1.0, 1.0)),
    (ViewPreset::IsoBackRight, Vec3::new(1.0, 1.0, 1.0)),
    (ViewPreset::IsoBackLeft, Vec3::new(-1.0, 1.0, 1.0)),
];

/// Corner pick radius in cube units.
pub const CORNER_RADIUS: f32 = 0.32;

fn project(camera: &OrbitCamera, p: Vec3) -> Vec2 {
    let b = camera.basis();
    Vec2::new(p.dot(b.right), p.dot(b.up))
}

/// Visible faces, back to front (paint in order).
pub fn visible_faces(camera: &OrbitCamera) -> Vec<CubeFace> {
    let eye = camera.eye_dir();
    let mut faces: Vec<CubeFace> = FACES
        .iter()
        .filter_map(|&(preset, label, n, r, u)| {
            let facing = n.dot(eye);
            if facing <= 0.02 {
                return None;
            }
            let corners = [n - r - u, n + r - u, n + r + u, n - r + u].map(|p| project(camera, p));
            Some(CubeFace { preset, label, corners, facing })
        })
        .collect();
    faces.sort_by(|a, b| a.facing.total_cmp(&b.facing));
    faces
}

/// Top corners that face the viewer.
pub fn visible_corners(camera: &OrbitCamera) -> Vec<CubeCorner> {
    let eye = camera.eye_dir();
    TOP_CORNERS
        .iter()
        .filter(|(_, p)| p.normalize().dot(eye) > 0.05)
        .map(|&(preset, p)| CubeCorner { preset, at: project(camera, p) })
        .collect()
}

/// What is under `point` (cube units, Y up). Corners win over faces.
pub fn hit(camera: &OrbitCamera, point: Vec2) -> Option<CubeHit> {
    if !point.is_finite() {
        return None;
    }
    if let Some(c) = visible_corners(camera)
        .into_iter()
        .filter(|c| c.at.distance(point) <= CORNER_RADIUS)
        .min_by(|a, b| a.at.distance(point).total_cmp(&b.at.distance(point)))
    {
        return Some(CubeHit::Corner(c.preset));
    }
    visible_faces(camera)
        .into_iter()
        .rev()
        .find(|f| point_in_convex(&f.corners, point))
        .map(|f| CubeHit::Face(f.preset))
}

fn point_in_convex(poly: &[Vec2; 4], p: Vec2) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let a = poly[i];
        let b = poly[(i + 1) % 4];
        let cross = (b - a).perp_dot(p - a);
        if cross.abs() < 1.0e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(preset: ViewPreset) -> OrbitCamera {
        let mut c = OrbitCamera::default();
        c.set_preset(preset);
        c
    }

    #[test]
    fn straight_views_show_one_face() {
        for (preset, label) in [
            (ViewPreset::Top, "TOP"),
            (ViewPreset::Front, "FRONT"),
            (ViewPreset::Right, "RIGHT"),
            (ViewPreset::Back, "BACK"),
            (ViewPreset::Left, "LEFT"),
            (ViewPreset::Bottom, "BOTTOM"),
        ] {
            let faces = visible_faces(&cam(preset));
            assert_eq!(faces.len(), 1, "{preset:?}");
            assert_eq!(faces[0].label, label);
        }
    }

    #[test]
    fn iso_view_shows_three_faces_and_centre_hits_the_corner() {
        let c = cam(ViewPreset::IsoFrontRight);
        assert_eq!(visible_faces(&c).len(), 3);
        let h = hit(&c, Vec2::ZERO).map(CubeHit::preset);
        assert_eq!(h, Some(ViewPreset::IsoFrontRight));
    }

    #[test]
    fn clicking_the_centre_of_a_straight_view_hits_that_face() {
        let c = cam(ViewPreset::Front);
        assert!(matches!(hit(&c, Vec2::ZERO), Some(CubeHit::Face(ViewPreset::Front))));
        assert!(hit(&c, Vec2::new(5.0, 5.0)).is_none());
        assert!(hit(&c, Vec2::NAN).is_none());
    }

    #[test]
    fn faces_paint_back_to_front() {
        let faces = visible_faces(&cam(ViewPreset::IsoBackLeft));
        assert!(faces.windows(2).all(|w| w[0].facing <= w[1].facing));
    }
}
