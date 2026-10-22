//! Offscreen wgpu renderer for the crystal viewport.
//!
//! This crate depends on wgpu only and knows nothing about GPUI: it renders
//! into its own textures and hands back BGRA pixels, which a presentation
//! layer then puts on screen (see `docs/design/viewport-rendering.md`).

mod camera;
mod renderer;
mod scene;

pub use camera::{CameraMatrices, OrbitCamera};
pub use renderer::{
    FrameImage, FrameTimings, PendingFrame, RenderError, RenderRequest, Renderer, srgb_to_linear,
};
pub use scene::{SphereInstance, synthetic_lattice};
