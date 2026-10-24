//! The 3D viewport: renders a scene offscreen with `mw-crystal-render`,
//! reads the pixels back and paints them as a GPUI image, so that GPUI
//! overlays can draw on top (see `docs/design/viewport-rendering.md`).
//!
//! The viewport owns the camera, the frame pipeline, picking and the hover
//! highlight. What to draw, what is selected, and any labels or toolbars
//! over it belong to the feature that hosts it.

rust_i18n::i18n!("locales", fallback = "en");

mod viewport;

pub use mw_crystal_render::{
    BondInstance, CameraPose, EllipsoidInstance, LineInstance, Pick, Polyhedron, Scene,
    SphereInstance, srgb_to_linear,
};
pub use viewport::{Snapshot, Viewport, ViewportEvent};
