//! L0 geometry primitives shared by every CosmicCAM crate.
//!
//! Units are millimetres, Z is up. The math types are re-exported from `glam`
//! so that every layer agrees on one vector and matrix representation.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
#![allow(non_snake_case)]

pub mod arcfit;
pub mod brep;
pub mod collide;
pub mod cutter;
pub mod edges;
pub mod faces;
pub mod kinematics;
pub mod mesh;
pub mod nurbs;
pub mod offset;
pub mod region;
pub mod trim;
pub mod voxel;
pub mod zmap;

pub use edges::{Edge, EdgeKind, EdgeMap};
pub use faces::{Face, FaceKind, FaceMap};
pub use glam::{DMat3, DMat4, DVec2, DVec3, Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
pub use mesh::{Mesh, MeshError};

/// World up axis. CosmicCAM is Z-up everywhere.
pub const UP: Vec3 = Vec3::Z;

/// Axis-aligned bounding box in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// An empty box that any `include` call will replace.
    pub const EMPTY: Self = Self { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) };

    pub fn new(a: Vec3, b: Vec3) -> Self {
        Self { min: a.min(b), max: a.max(b) }
    }

    /// Builds the box around `points`, skipping non-finite points.
    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Self {
        let mut aabb = Self::EMPTY;
        for p in points {
            aabb.include(p);
        }
        aabb
    }

    pub fn include(&mut self, p: Vec3) {
        if p.is_finite() {
            self.min = self.min.min(p);
            self.max = self.max.max(p);
        }
    }

    pub fn is_empty(&self) -> bool {
        !(self.min.x <= self.max.x && self.min.y <= self.max.y && self.min.z <= self.max.z)
    }

    pub fn center(&self) -> Vec3 {
        if self.is_empty() { Vec3::ZERO } else { (self.min + self.max) * 0.5 }
    }

    pub fn size(&self) -> Vec3 {
        if self.is_empty() { Vec3::ZERO } else { self.max - self.min }
    }

    /// Radius of the sphere through the corners; zero for an empty box.
    pub fn radius(&self) -> f32 {
        self.size().length() * 0.5
    }
}

impl Default for Aabb {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_box_reports_empty_and_zero_size() {
        let b = Aabb::EMPTY;
        assert!(b.is_empty());
        assert_eq!(b.size(), Vec3::ZERO);
        assert_eq!(b.center(), Vec3::ZERO);
    }

    #[test]
    fn from_points_skips_non_finite() {
        let b = Aabb::from_points([Vec3::new(1.0, 2.0, 3.0), Vec3::new(f32::NAN, 0.0, 0.0), Vec3::new(-1.0, 0.0, 5.0)]);
        assert_eq!(b.min, Vec3::new(-1.0, 0.0, 3.0));
        assert_eq!(b.max, Vec3::new(1.0, 2.0, 5.0));
        assert_eq!(b.center(), Vec3::new(0.0, 1.0, 4.0));
    }
}
