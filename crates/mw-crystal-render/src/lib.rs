//! Offscreen wgpu renderer for the crystal viewport.
//!
//! This crate depends on wgpu only and knows nothing about GPUI: it renders
//! a [`Scene`] of impostor spheres and ellipsoids, impostor-cylinder bonds,
//! lines and translucent polyhedra into its own textures and hands back BGRA pixels,
//! which a presentation layer then puts on screen. A separate ID texture answers which sphere or bond lies
//! under a pixel (see `docs/design/viewport-rendering.md`).

mod camera;
mod renderer;
mod scene;

pub use camera::{CameraMatrices, CameraPose, OrbitCamera};
pub use renderer::{
    FrameImage, FrameTimings, PendingFrame, PendingPick, Pick, PickRequest, RenderError,
    RenderRequest, Renderer, linear_to_srgb, srgb_to_linear,
};
pub use scene::{
    BondInstance, EllipsoidInstance, HighlightRing, LineInstance, Polyhedron, Scene,
    SphereInstance, synthetic_bonds, synthetic_lattice, synthetic_polyhedra,
};
