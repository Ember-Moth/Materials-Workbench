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

/// Element colors (Jmol, sRGB) and covalent radii in Å (Cordero et al.
/// 2008; 1.50 where none is tabulated). These are data colors, not theme
/// colors.
const ELEMENTS: [(&str, [u8; 3], f32); 109] = [
    ("H", [0xFF, 0xFF, 0xFF], 0.31),
    ("He", [0xD9, 0xFF, 0xFF], 0.28),
    ("Li", [0xCC, 0x80, 0xFF], 1.28),
    ("Be", [0xC2, 0xFF, 0x00], 0.96),
    ("B", [0xFF, 0xB5, 0xB5], 0.84),
    ("C", [0x90, 0x90, 0x90], 0.76),
    ("N", [0x30, 0x50, 0xF8], 0.71),
    ("O", [0xFF, 0x0D, 0x0D], 0.66),
    ("F", [0x90, 0xE0, 0x50], 0.57),
    ("Ne", [0xB3, 0xE3, 0xF5], 0.58),
    ("Na", [0xAB, 0x5C, 0xF2], 1.66),
    ("Mg", [0x8A, 0xFF, 0x00], 1.41),
    ("Al", [0xBF, 0xA6, 0xA6], 1.21),
    ("Si", [0xF0, 0xC8, 0xA0], 1.11),
    ("P", [0xFF, 0x80, 0x00], 1.07),
    ("S", [0xFF, 0xFF, 0x30], 1.05),
    ("Cl", [0x1F, 0xF0, 0x1F], 1.02),
    ("Ar", [0x80, 0xD1, 0xE3], 1.06),
    ("K", [0x8F, 0x40, 0xD4], 2.03),
    ("Ca", [0x3D, 0xFF, 0x00], 1.76),
    ("Sc", [0xE6, 0xE6, 0xE6], 1.70),
    ("Ti", [0xBF, 0xC2, 0xC7], 1.60),
    ("V", [0xA6, 0xA6, 0xAB], 1.53),
    ("Cr", [0x8A, 0x99, 0xC7], 1.39),
    ("Mn", [0x9C, 0x7A, 0xC7], 1.39),
    ("Fe", [0xE0, 0x66, 0x33], 1.32),
    ("Co", [0xF0, 0x90, 0xA0], 1.26),
    ("Ni", [0x50, 0xD0, 0x50], 1.24),
    ("Cu", [0xC8, 0x80, 0x33], 1.32),
    ("Zn", [0x7D, 0x80, 0xB0], 1.22),
    ("Ga", [0xC2, 0x8F, 0x8F], 1.22),
    ("Ge", [0x66, 0x8F, 0x8F], 1.20),
    ("As", [0xBD, 0x80, 0xE3], 1.19),
    ("Se", [0xFF, 0xA1, 0x00], 1.20),
    ("Br", [0xA6, 0x29, 0x29], 1.20),
    ("Kr", [0x5C, 0xB8, 0xD1], 1.16),
    ("Rb", [0x70, 0x2E, 0xB0], 2.20),
    ("Sr", [0x00, 0xFF, 0x00], 1.95),
    ("Y", [0x94, 0xFF, 0xFF], 1.90),
    ("Zr", [0x94, 0xE0, 0xE0], 1.75),
    ("Nb", [0x73, 0xC2, 0xC9], 1.64),
    ("Mo", [0x54, 0xB5, 0xB5], 1.54),
    ("Tc", [0x3B, 0x9E, 0x9E], 1.47),
    ("Ru", [0x24, 0x8F, 0x8F], 1.46),
    ("Rh", [0x0A, 0x7D, 0x8C], 1.42),
    ("Pd", [0x00, 0x69, 0x85], 1.39),
    ("Ag", [0xC0, 0xC0, 0xC0], 1.45),
    ("Cd", [0xFF, 0xD9, 0x8F], 1.44),
    ("In", [0xA6, 0x75, 0x73], 1.42),
    ("Sn", [0x66, 0x80, 0x80], 1.39),
    ("Sb", [0x9E, 0x63, 0xB5], 1.39),
    ("Te", [0xD4, 0x7A, 0x00], 1.38),
    ("I", [0x94, 0x00, 0x94], 1.39),
    ("Xe", [0x42, 0x9E, 0xB0], 1.40),
    ("Cs", [0x57, 0x17, 0x8F], 2.44),
    ("Ba", [0x00, 0xC9, 0x00], 2.15),
    ("La", [0x70, 0xD4, 0xFF], 2.07),
    ("Ce", [0xFF, 0xFF, 0xC7], 2.04),
    ("Pr", [0xD9, 0xFF, 0xC7], 2.03),
    ("Nd", [0xC7, 0xFF, 0xC7], 2.01),
    ("Pm", [0xA3, 0xFF, 0xC7], 1.99),
    ("Sm", [0x8F, 0xFF, 0xC7], 1.98),
    ("Eu", [0x61, 0xFF, 0xC7], 1.98),
    ("Gd", [0x45, 0xFF, 0xC7], 1.96),
    ("Tb", [0x30, 0xFF, 0xC7], 1.94),
    ("Dy", [0x1F, 0xFF, 0xC7], 1.92),
    ("Ho", [0x00, 0xFF, 0x9C], 1.92),
    ("Er", [0x00, 0xE6, 0x75], 1.89),
    ("Tm", [0x00, 0xD4, 0x52], 1.90),
    ("Yb", [0x00, 0xBF, 0x38], 1.87),
    ("Lu", [0x00, 0xAB, 0x24], 1.87),
    ("Hf", [0x4D, 0xC2, 0xFF], 1.75),
    ("Ta", [0x4D, 0xA6, 0xFF], 1.70),
    ("W", [0x21, 0x94, 0xD6], 1.62),
    ("Re", [0x26, 0x7D, 0xAB], 1.51),
    ("Os", [0x26, 0x66, 0x96], 1.44),
    ("Ir", [0x17, 0x54, 0x87], 1.41),
    ("Pt", [0xD0, 0xD0, 0xE0], 1.36),
    ("Au", [0xFF, 0xD1, 0x23], 1.36),
    ("Hg", [0xB8, 0xB8, 0xD0], 1.32),
    ("Tl", [0xA6, 0x54, 0x4D], 1.45),
    ("Pb", [0x57, 0x59, 0x61], 1.46),
    ("Bi", [0x9E, 0x4F, 0xB5], 1.48),
    ("Po", [0xAB, 0x5C, 0x00], 1.40),
    ("At", [0x75, 0x4F, 0x45], 1.50),
    ("Rn", [0x42, 0x82, 0x96], 1.50),
    ("Fr", [0x42, 0x00, 0x66], 2.60),
    ("Ra", [0x00, 0x7D, 0x00], 2.21),
    ("Ac", [0x70, 0xAB, 0xFA], 2.15),
    ("Th", [0x00, 0xBA, 0xFF], 2.06),
    ("Pa", [0x00, 0xA1, 0xFF], 2.00),
    ("U", [0x00, 0x8F, 0xFF], 1.96),
    ("Np", [0x00, 0x80, 0xFF], 1.90),
    ("Pu", [0x00, 0x6B, 0xFF], 1.87),
    ("Am", [0x54, 0x5C, 0xF2], 1.80),
    ("Cm", [0x78, 0x5C, 0xE3], 1.69),
    ("Bk", [0x8A, 0x4F, 0xE3], 1.50),
    ("Cf", [0xA1, 0x36, 0xD4], 1.50),
    ("Es", [0xB3, 0x1F, 0xD4], 1.50),
    ("Fm", [0xB3, 0x1F, 0xBA], 1.50),
    ("Md", [0xB3, 0x0D, 0xA6], 1.50),
    ("No", [0xBD, 0x0D, 0x87], 1.50),
    ("Lr", [0xC7, 0x00, 0x66], 1.50),
    ("Rf", [0xCC, 0x00, 0x59], 1.50),
    ("Db", [0xD1, 0x00, 0x4F], 1.50),
    ("Sg", [0xD9, 0x00, 0x45], 1.50),
    ("Bh", [0xE0, 0x00, 0x38], 1.50),
    ("Hs", [0xE6, 0x00, 0x2E], 1.50),
    ("Mt", [0xEB, 0x00, 0x26], 1.50),
];

/// Jmol's color for anything that is not an element, such as `X`.
const UNKNOWN_COLOR: [u8; 3] = [0xFF, 0x14, 0x93];
const UNKNOWN_RADIUS: f32 = 1.50;

/// Color and ball-and-stick radius in Å. The radius grows with the covalent
/// radius but less than in proportion, so that light atoms stay visible
/// beside heavy ones (O 0.38, Ti 0.57, Cs 0.74).
fn element_style(symbol: &str) -> ([u8; 3], f32) {
    let (color, covalent) = ELEMENTS
        .iter()
        .find_map(|(s, color, radius)| (*s == symbol).then_some((*color, *radius)))
        .unwrap_or((UNKNOWN_COLOR, UNKNOWN_RADIUS));
    (color, 0.25 + 0.2 * covalent)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_symbol_is_an_element() {
        for (symbol, _, _) in ELEMENTS {
            assert!(mw_crystal::element::is_element(symbol), "{symbol}");
        }
        assert_eq!(element_style("O").0, [0xFF, 0x0D, 0x0D]);
        assert_eq!(element_style(mw_crystal::UNKNOWN_ELEMENT).0, UNKNOWN_COLOR);
    }
}
