//! Offscreen wgpu renderer for the crystal viewport.
//!
//! This crate depends on wgpu only and knows nothing about GPUI: it renders
//! a [`Scene`] of impostor spheres, impostor-cylinder bonds and lines into its
//! own textures and hands back BGRA pixels, which a presentation layer then
//! puts on screen. A separate ID texture answers which sphere or bond lies
//! under a pixel (see `docs/design/viewport-rendering.md`).

mod camera;
mod renderer;
mod scene;

pub use camera::{CameraMatrices, OrbitCamera};
pub use renderer::{
    FrameImage, FrameTimings, PendingFrame, PendingPick, Pick, PickRequest, RenderError,
    RenderRequest, Renderer, srgb_to_linear,
};
pub use scene::{
    BondInstance, HighlightRing, LineInstance, Scene, SphereInstance, synthetic_bonds,
    synthetic_lattice,
};
