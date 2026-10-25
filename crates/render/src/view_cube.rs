//! View cube geometry: which faces and corners face the viewer and where they land on screen.
//!
//! Coordinates are in "cube units" with the cube spanning -1..1 and screen Y pointing up;
//! the UI scales and flips them into its overlay rectangle.

use CosmicCAM_geom::{Vec2, Vec3};

use crate::camera::OrbitCamera;

/// What part of the chamfered cube a facet is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetKind {
    /// One of the six labelled faces: a straight view.
    Face,
    /// A chamfer along one of the twelve edges: a view between two faces.
    Edge,
    /// A triangle cut off one of the eight corners: an isometric view.
    Corner,
}

/// One clickable facet of the cube, projected onto the screen.
#[derive(Clone, Debug)]
pub struct CubeFacet {
    /// Stable index (0..26), for hover tracking.
    pub id: u8,
    pub kind: FacetKind,
    /// Face name; empty for edges and corners.
    pub label: &'static str,
    /// Outward unit normal: clicking the facet looks along it.
    pub normal: Vec3,
    /// Screen outline (cube units, Y up), convex.
    pub corners: Vec<Vec2>,
    /// How squarely the facet points at the viewer, 0..1.
    pub facing: f32,
}

const FACES: [(&str, Vec3, Vec3, Vec3); 6] = [
    // label, normal, in-face "right", in-face "up"
    ("TOP", Vec3::Z, Vec3::X, Vec3::Y),
    ("BOTTOM", Vec3::NEG_Z, Vec3::X, Vec3::NEG_Y),
    ("FRONT", Vec3::NEG_Y, Vec3::X, Vec3::Z),
    ("BACK", Vec3::Y, Vec3::NEG_X, Vec3::Z),
    ("RIGHT", Vec3::X, Vec3::Y, Vec3::Z),
    ("LEFT", Vec3::NEG_X, Vec3::NEG_Y, Vec3::Z),
];

/// Width of the edge and corner chamfers, in cube units (the cube spans -1..1).
pub const CHAMFER: f32 = 0.3;

/// Every facet in cube space: (kind, label, outward normal, outline).
fn facets_3d() -> Vec<(FacetKind, &'static str, Vec3, Vec<Vec3>)> {
    let k = 1.0 - CHAMFER;
    let mut out = Vec::with_capacity(26);
    for (label, n, r, u) in FACES {
        let corners = vec![n + k * (-r - u), n + k * (r - u), n + k * (r + u), n + k * (-r + u)];
        out.push((FacetKind::Face, label, n, corners));
    }
    // Edges run along axis `a`, between the faces on `b` and `c`.
    for (a, b, c) in [(Vec3::X, Vec3::Y, Vec3::Z), (Vec3::Y, Vec3::Z, Vec3::X), (Vec3::Z, Vec3::X, Vec3::Y)] {
        for (sb, sc) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            let (n1, n2) = (b * sb, c * sc);
            let corners = vec![n1 + k * n2 + k * a, n1 + k * n2 - k * a, n2 + k * n1 - k * a, n2 + k * n1 + k * a];
            out.push((FacetKind::Edge, "", (n1 + n2).normalize(), corners));
        }
    }
    for sx in [1.0, -1.0] {
        for sy in [1.0, -1.0] {
            for sz in [1.0, -1.0] {
                let corners =
                    vec![Vec3::new(sx, k * sy, k * sz), Vec3::new(k * sx, sy, k * sz), Vec3::new(k * sx, k * sy, sz)];
                out.push((FacetKind::Corner, "", Vec3::new(sx, sy, sz).normalize(), corners));
            }
        }
    }
    out
}

fn project(camera: &OrbitCamera, p: Vec3) -> Vec2 {
    let b = camera.basis();
    Vec2::new(p.dot(b.right), p.dot(b.up))
}

/// Facets that face the viewer, back to front (paint in order).
pub fn visible_facets(camera: &OrbitCamera) -> Vec<CubeFacet> {
    let eye = camera.eye_dir();
    let mut facets: Vec<CubeFacet> = facets_3d()
        .into_iter()
        .enumerate()
        .filter_map(|(id, (kind, label, normal, corners))| {
            let facing = normal.dot(eye);
            if facing <= 0.02 {
                return None;
            }
            let corners = corners.into_iter().map(|p| project(camera, p)).collect();
            Some(CubeFacet { id: id as u8, kind, label, normal, corners, facing })
        })
        .collect();
    facets.sort_by(|a, b| a.facing.total_cmp(&b.facing));
    facets
}

/// The facet under `point` (cube units, Y up), if any.
pub fn hit(camera: &OrbitCamera, point: Vec2) -> Option<CubeFacet> {
    if !point.is_finite() {
        return None;
    }
    visible_facets(camera).into_iter().rev().find(|f| point_in_convex(&f.corners, point))
}

/// Camera (yaw, pitch) in radians that looks along `normal` towards the cube. Straight up
/// and down views keep the front at the bottom of the screen, like the Top preset.
pub fn view_angles(normal: Vec3) -> (f32, f32) {
    let n = normal.normalize_or_zero();
    let pitch = n.z.clamp(-1.0, 1.0).asin();
    let yaw = if n.x.hypot(n.y) < 1.0e-4 { -std::f32::consts::FRAC_PI_2 } else { n.y.atan2(n.x) };
    (yaw, pitch)
}

fn point_in_convex(poly: &[Vec2], p: Vec2) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut sign = 0.0f32;
    for (i, &a) in poly.iter().enumerate() {
        let b = poly.get((i + 1) % poly.len()).copied().unwrap_or(a);
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
    use crate::camera::ViewPreset;

    fn cam(preset: ViewPreset) -> OrbitCamera {
        let mut c = OrbitCamera::default();
        c.set_preset(preset);
        c
    }

    #[test]
    fn the_cube_has_six_faces_twelve_edges_and_eight_corners() {
        let all = facets_3d();
        assert_eq!(all.len(), 26);
        for (kind, n) in [(FacetKind::Face, 6), (FacetKind::Edge, 12), (FacetKind::Corner, 8)] {
            assert_eq!(all.iter().filter(|f| f.0 == kind).count(), n);
        }
        // Every facet is flat and faces outwards.
        for (_, _, normal, corners) in &all {
            let d = corners[0].dot(*normal);
            assert!(d > 0.5);
            assert!(corners.iter().all(|c| (c.dot(*normal) - d).abs() < 1.0e-5));
        }
    }

    #[test]
    fn straight_views_centre_on_their_face() {
        for (preset, label) in [
            (ViewPreset::Top, "TOP"),
            (ViewPreset::Front, "FRONT"),
            (ViewPreset::Right, "RIGHT"),
            (ViewPreset::Back, "BACK"),
            (ViewPreset::Left, "LEFT"),
            (ViewPreset::Bottom, "BOTTOM"),
        ] {
            let c = cam(preset);
            let faces: Vec<_> = visible_facets(&c).into_iter().filter(|f| f.kind == FacetKind::Face).collect();
            assert_eq!(faces.len(), 1, "{preset:?}");
            assert_eq!(faces[0].label, label);
            assert_eq!(hit(&c, Vec2::ZERO).map(|f| f.label), Some(label));
        }
    }

    #[test]
    fn iso_view_centre_hits_the_corner_and_its_angles_match_the_preset() {
        let c = cam(ViewPreset::IsoFrontRight);
        let corner = hit(&c, Vec2::ZERO);
        assert_eq!(corner.as_ref().map(|f| f.kind), Some(FacetKind::Corner));
        let (yaw, pitch) = view_angles(corner.map_or(Vec3::ZERO, |f| f.normal));
        let (want_yaw, want_pitch) = ViewPreset::IsoFrontRight.angles();
        assert!((yaw - want_yaw).abs() < 1.0e-4 && (pitch - want_pitch).abs() < 1.0e-4);
    }

    #[test]
    fn edges_look_between_faces_and_top_looks_down_from_the_front() {
        let (yaw, pitch) = view_angles(Vec3::new(0.0, -1.0, 1.0));
        assert!((yaw + std::f32::consts::FRAC_PI_2).abs() < 1.0e-4);
        assert!((pitch - std::f32::consts::FRAC_PI_4).abs() < 1.0e-4);
        assert_eq!(view_angles(Vec3::Z), ViewPreset::Top.angles());
        assert!(view_angles(Vec3::ZERO).0.is_finite());
    }

    #[test]
    fn misses_and_bad_points_hit_nothing() {
        let c = cam(ViewPreset::Front);
        assert!(hit(&c, Vec2::new(5.0, 5.0)).is_none());
        assert!(hit(&c, Vec2::NAN).is_none());
    }

    #[test]
    fn facets_paint_back_to_front() {
        let facets = visible_facets(&cam(ViewPreset::IsoBackLeft));
        assert!(facets.windows(2).all(|w| w[0].facing <= w[1].facing));
    }
}
