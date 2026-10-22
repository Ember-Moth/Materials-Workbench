use gpui_kit::{Hsla, Rgba};
use mw_crystal::SiteId;
use mw_viewport::{SphereInstance, srgb_to_linear};

use crate::document::StructureDocument;

/// How atoms are drawn.
///
/// Mock: only the radius scale differs so far; polyhedra need the bond and
/// polygon passes the renderer does not have yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayModel {
    BallAndStick,
    SpaceFilling,
    Polyhedra,
}

impl DisplayModel {
    fn radius_scale(self) -> f32 {
        match self {
            Self::BallAndStick | Self::Polyhedra => 1.0,
            Self::SpaceFilling => 2.6,
        }
    }
}

/// Element colors (Jmol, sRGB) and ball-and-stick radii in Å. These are
/// data colors, not theme colors.
fn element_style(symbol: &str) -> ([u8; 3], f32) {
    match symbol {
        "O" => ([0xFF, 0x0D, 0x0D], 0.35),
        "Ti" => ([0xBF, 0xC2, 0xC7], 0.5),
        "Sr" => ([0x00, 0xFF, 0x00], 0.65),
        _ => ([0xFF, 0x14, 0x93], 0.4),
    }
}

/// The spheres for a document.
///
/// Mock: atoms of the selected site are tinted with `highlight` (the theme's
/// selection color) until the renderer can draw a selection outline.
pub(crate) fn spheres(
    document: &StructureDocument,
    selected: Option<SiteId>,
    model: DisplayModel,
    highlight: Hsla,
) -> Vec<SphereInstance> {
    let highlight = Rgba::from(highlight);
    let highlight = [highlight.r, highlight.g, highlight.b].map(srgb_to_linear);
    document
        .derived
        .atoms
        .iter()
        .map(|atom| {
            let (srgb, radius) = element_style(&atom.element);
            let mut color = srgb.map(|c| srgb_to_linear(f32::from(c) / 255.0));
            if selected == Some(atom.site) {
                for (c, h) in color.iter_mut().zip(highlight) {
                    *c = *c * 0.45 + h * 0.55;
                }
            }
            let [x, y, z] = atom.position.map(|v| v as f32);
            SphereInstance {
                center_radius: [x, y, z, radius * model.radius_scale()],
                color: [color[0], color[1], color[2], 1.0],
            }
        })
        .collect()
}
