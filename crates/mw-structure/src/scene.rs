use gpui_kit::{Hsla, Rgba};
use mw_crystal::covalent_radius;
use mw_viewport::{BondInstance, LineInstance, Scene, SphereInstance, srgb_to_linear};

use crate::document::StructureDocument;

/// How atoms are drawn.
///
/// Mock: polyhedra need the polygon pass the renderer does not have yet;
/// they draw as ball and stick.
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

    /// Space filling hides bonds inside the atoms, so they are not drawn.
    fn shows_bonds(self) -> bool {
        self != Self::SpaceFilling
    }
}

/// Element colors (Jmol, sRGB). These are data colors, not theme colors.
const ELEMENTS: [(&str, [u8; 3]); 109] = [
    ("H", [0xFF, 0xFF, 0xFF]),
    ("He", [0xD9, 0xFF, 0xFF]),
    ("Li", [0xCC, 0x80, 0xFF]),
    ("Be", [0xC2, 0xFF, 0x00]),
    ("B", [0xFF, 0xB5, 0xB5]),
    ("C", [0x90, 0x90, 0x90]),
    ("N", [0x30, 0x50, 0xF8]),
    ("O", [0xFF, 0x0D, 0x0D]),
    ("F", [0x90, 0xE0, 0x50]),
    ("Ne", [0xB3, 0xE3, 0xF5]),
    ("Na", [0xAB, 0x5C, 0xF2]),
    ("Mg", [0x8A, 0xFF, 0x00]),
    ("Al", [0xBF, 0xA6, 0xA6]),
    ("Si", [0xF0, 0xC8, 0xA0]),
    ("P", [0xFF, 0x80, 0x00]),
    ("S", [0xFF, 0xFF, 0x30]),
    ("Cl", [0x1F, 0xF0, 0x1F]),
    ("Ar", [0x80, 0xD1, 0xE3]),
    ("K", [0x8F, 0x40, 0xD4]),
    ("Ca", [0x3D, 0xFF, 0x00]),
    ("Sc", [0xE6, 0xE6, 0xE6]),
    ("Ti", [0xBF, 0xC2, 0xC7]),
    ("V", [0xA6, 0xA6, 0xAB]),
    ("Cr", [0x8A, 0x99, 0xC7]),
    ("Mn", [0x9C, 0x7A, 0xC7]),
    ("Fe", [0xE0, 0x66, 0x33]),
    ("Co", [0xF0, 0x90, 0xA0]),
    ("Ni", [0x50, 0xD0, 0x50]),
    ("Cu", [0xC8, 0x80, 0x33]),
    ("Zn", [0x7D, 0x80, 0xB0]),
    ("Ga", [0xC2, 0x8F, 0x8F]),
    ("Ge", [0x66, 0x8F, 0x8F]),
    ("As", [0xBD, 0x80, 0xE3]),
    ("Se", [0xFF, 0xA1, 0x00]),
    ("Br", [0xA6, 0x29, 0x29]),
    ("Kr", [0x5C, 0xB8, 0xD1]),
    ("Rb", [0x70, 0x2E, 0xB0]),
    ("Sr", [0x00, 0xFF, 0x00]),
    ("Y", [0x94, 0xFF, 0xFF]),
    ("Zr", [0x94, 0xE0, 0xE0]),
    ("Nb", [0x73, 0xC2, 0xC9]),
    ("Mo", [0x54, 0xB5, 0xB5]),
    ("Tc", [0x3B, 0x9E, 0x9E]),
    ("Ru", [0x24, 0x8F, 0x8F]),
    ("Rh", [0x0A, 0x7D, 0x8C]),
    ("Pd", [0x00, 0x69, 0x85]),
    ("Ag", [0xC0, 0xC0, 0xC0]),
    ("Cd", [0xFF, 0xD9, 0x8F]),
    ("In", [0xA6, 0x75, 0x73]),
    ("Sn", [0x66, 0x80, 0x80]),
    ("Sb", [0x9E, 0x63, 0xB5]),
    ("Te", [0xD4, 0x7A, 0x00]),
    ("I", [0x94, 0x00, 0x94]),
    ("Xe", [0x42, 0x9E, 0xB0]),
    ("Cs", [0x57, 0x17, 0x8F]),
    ("Ba", [0x00, 0xC9, 0x00]),
    ("La", [0x70, 0xD4, 0xFF]),
    ("Ce", [0xFF, 0xFF, 0xC7]),
    ("Pr", [0xD9, 0xFF, 0xC7]),
    ("Nd", [0xC7, 0xFF, 0xC7]),
    ("Pm", [0xA3, 0xFF, 0xC7]),
    ("Sm", [0x8F, 0xFF, 0xC7]),
    ("Eu", [0x61, 0xFF, 0xC7]),
    ("Gd", [0x45, 0xFF, 0xC7]),
    ("Tb", [0x30, 0xFF, 0xC7]),
    ("Dy", [0x1F, 0xFF, 0xC7]),
    ("Ho", [0x00, 0xFF, 0x9C]),
    ("Er", [0x00, 0xE6, 0x75]),
    ("Tm", [0x00, 0xD4, 0x52]),
    ("Yb", [0x00, 0xBF, 0x38]),
    ("Lu", [0x00, 0xAB, 0x24]),
    ("Hf", [0x4D, 0xC2, 0xFF]),
    ("Ta", [0x4D, 0xA6, 0xFF]),
    ("W", [0x21, 0x94, 0xD6]),
    ("Re", [0x26, 0x7D, 0xAB]),
    ("Os", [0x26, 0x66, 0x96]),
    ("Ir", [0x17, 0x54, 0x87]),
    ("Pt", [0xD0, 0xD0, 0xE0]),
    ("Au", [0xFF, 0xD1, 0x23]),
    ("Hg", [0xB8, 0xB8, 0xD0]),
    ("Tl", [0xA6, 0x54, 0x4D]),
    ("Pb", [0x57, 0x59, 0x61]),
    ("Bi", [0x9E, 0x4F, 0xB5]),
    ("Po", [0xAB, 0x5C, 0x00]),
    ("At", [0x75, 0x4F, 0x45]),
    ("Rn", [0x42, 0x82, 0x96]),
    ("Fr", [0x42, 0x00, 0x66]),
    ("Ra", [0x00, 0x7D, 0x00]),
    ("Ac", [0x70, 0xAB, 0xFA]),
    ("Th", [0x00, 0xBA, 0xFF]),
    ("Pa", [0x00, 0xA1, 0xFF]),
    ("U", [0x00, 0x8F, 0xFF]),
    ("Np", [0x00, 0x80, 0xFF]),
    ("Pu", [0x00, 0x6B, 0xFF]),
    ("Am", [0x54, 0x5C, 0xF2]),
    ("Cm", [0x78, 0x5C, 0xE3]),
    ("Bk", [0x8A, 0x4F, 0xE3]),
    ("Cf", [0xA1, 0x36, 0xD4]),
    ("Es", [0xB3, 0x1F, 0xD4]),
    ("Fm", [0xB3, 0x1F, 0xBA]),
    ("Md", [0xB3, 0x0D, 0xA6]),
    ("No", [0xBD, 0x0D, 0x87]),
    ("Lr", [0xC7, 0x00, 0x66]),
    ("Rf", [0xCC, 0x00, 0x59]),
    ("Db", [0xD1, 0x00, 0x4F]),
    ("Sg", [0xD9, 0x00, 0x45]),
    ("Bh", [0xE0, 0x00, 0x38]),
    ("Hs", [0xE6, 0x00, 0x2E]),
    ("Mt", [0xEB, 0x00, 0x26]),
];

/// Jmol's color for anything that is not an element, such as `X`.
const UNKNOWN_COLOR: [u8; 3] = [0xFF, 0x14, 0x93];
/// Å, for elements without a tabulated covalent radius.
const UNKNOWN_RADIUS: f64 = 1.50;
/// Å, the cylinder radius of bonds in ball and stick.
const BOND_RADIUS: f32 = 0.12;
/// Logical pixels.
const CELL_EDGE_WIDTH: f32 = 1.25;
const MEASUREMENT_WIDTH: f32 = 1.5;
const MEASUREMENT_DASH: f32 = 4.0;

/// Color and ball-and-stick radius in Å. The radius grows with the covalent
/// radius but less than in proportion, so that light atoms stay visible
/// beside heavy ones (O 0.38, Ti 0.57, Cs 0.74).
fn element_style(symbol: &str) -> ([u8; 3], f32) {
    let color = ELEMENTS
        .iter()
        .find_map(|(s, color)| (*s == symbol).then_some(*color))
        .unwrap_or(UNKNOWN_COLOR);
    let covalent = covalent_radius(symbol).unwrap_or(UNKNOWN_RADIUS);
    (color, 0.25 + 0.2 * covalent as f32)
}

fn linear(color: Hsla) -> [f32; 4] {
    let rgba = Rgba::from(color);
    let [r, g, b] = [rgba.r, rgba.g, rgba.b].map(srgb_to_linear);
    [r, g, b, 1.0]
}

fn point(p: [f64; 3]) -> [f32; 3] {
    p.map(|v| v as f32)
}

/// Theme colors of the lines drawn into the scene.
pub(crate) struct LineColors {
    pub cell: Hsla,
    pub measurement: Hsla,
}

/// What the viewport draws for a document: one sphere per displayed atom,
/// in the order of `derived.atoms`, so a picked sphere's index is the
/// atom's index; the bonds between displayed atoms; the unit-cell edges;
/// and a dashed line along each measurement.
pub(crate) fn scene(
    document: &StructureDocument,
    model: DisplayModel,
    colors: LineColors,
) -> Scene {
    let derived = document.derived();
    let lattice = &document.structure().lattice;
    let spheres: Vec<SphereInstance> = derived
        .atoms
        .iter()
        .map(|atom| {
            let (srgb, radius) = element_style(&atom.element);
            let [r, g, b] = srgb.map(|c| srgb_to_linear(f32::from(c) / 255.0));
            let [x, y, z] = point(atom.position);
            SphereInstance {
                center_radius: [x, y, z, radius * model.radius_scale()],
                color: [r, g, b, 1.0],
            }
        })
        .collect();

    let bonds = if model.shows_bonds() {
        derived
            .display_bonds
            .iter()
            .map(|&[i, j]| {
                let (a, b) = (&spheres[i], &spheres[j]);
                let mut start_radius = a.center_radius;
                start_radius[3] = BOND_RADIUS;
                BondInstance {
                    start_radius,
                    end: b.center_radius,
                    start_color: a.color,
                    end_color: b.color,
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    let line = |from: [f64; 3], to: [f64; 3], width: f32, dash: f32, color: [f32; 4]| {
        let ([x, y, z], [ex, ey, ez]) = (point(from), point(to));
        LineInstance {
            start_width: [x, y, z, width],
            end_dash: [ex, ey, ez, dash],
            color,
        }
    };
    let cell_color = linear(colors.cell);
    let corner = |i: usize| lattice.to_cartesian([0, 1, 2].map(|axis| (i >> axis & 1) as f64));
    let mut lines: Vec<LineInstance> = (0..8)
        .flat_map(|i| [1, 2, 4].map(move |bit| (i, i | bit)))
        .filter(|(i, j)| i != j)
        .map(|(i, j)| line(corner(i), corner(j), CELL_EDGE_WIDTH, 0.0, cell_color))
        .collect();
    let measurement_color = linear(colors.measurement);
    // Stale measurements may refer to atoms that are gone or moved.
    for annotation in document.crystal.annotations().iter().filter(|a| !a.stale) {
        let measurement = &annotation.measurement;
        let points: Option<Vec<[f64; 3]>> = measurement
            .atoms()
            .iter()
            .map(|atom| derived.cartesian(lattice, *atom))
            .collect();
        let Some(points) = points else {
            continue;
        };
        lines.extend(points.windows(2).map(|pair| {
            line(
                pair[0],
                pair[1],
                MEASUREMENT_WIDTH,
                MEASUREMENT_DASH,
                measurement_color,
            )
        }));
    }

    Scene {
        spheres,
        bonds,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_symbol_is_an_element() {
        for (symbol, _) in ELEMENTS {
            assert!(mw_crystal::element::is_element(symbol), "{symbol}");
        }
        assert_eq!(element_style("O").0, [0xFF, 0x0D, 0x0D]);
        assert_eq!(element_style(mw_crystal::UNKNOWN_ELEMENT).0, UNKNOWN_COLOR);
    }
}
