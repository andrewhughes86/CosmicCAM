//! L1 rendering model: cameras, scene buffers and viewport overlays as plain data.
//!
//! Nothing here talks to a GPU. The UI layer owns the backend (wgpu) and uploads
//! [`scene::SceneGeometry`] buffers as they are.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
#![allow(non_snake_case)]

pub mod camera;
pub mod scene;
pub mod view_cube;

pub use camera::{OrbitCamera, Projection, ViewPreset};
pub use scene::{SceneGeometry, ScenePalette, Vertex};
