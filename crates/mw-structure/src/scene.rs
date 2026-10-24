use std::collections::HashMap;

use gpui_kit::{Hsla, Rgba};
use mw_crystal::{
    Annotation, AtomFate, AtomRef, BondRules, DisplayRange, Displayed, IdMap, Orbit,
    PolyhedronCenters, Structure, covalent_radius,
};
use mw_viewport::{BondInstance, LineInstance, Polyhedron, Scene, SphereInstance, srgb_to_linear};

/// How atoms are drawn. Polyhedra draw the atoms as ball and stick, with
/// the coordination polyhedra of the chosen centers over them
/// (crystallography design §5.6).
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
/// Logical pixels: thinner than the cell's edges, which stay the frame.
const POLYHEDRON_EDGE_WIDTH: f32 = 1.0;
/// How much of what is behind a polyhedron's face shows through it.
const POLYHEDRON_OPACITY: f32 = 0.5;
/// The edges of a polyhedron in its center's color, darkened so that they
/// stand out from the faces.
const POLYHEDRON_EDGE_SHADE: f32 = 0.45;
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

/// The atoms of a structure as drawn: its orbits, and the atoms its display
/// range shows with the bonds between them.
pub(crate) struct Atoms<'a> {
    pub structure: &'a Structure,
    pub orbits: &'a [Orbit],
    pub displayed: &'a Displayed,
    /// How references in the document's measurements map to these atoms;
    /// `None` for the document's own.
    pub ids: Option<&'a IdMap>,
}

impl<'a> Atoms<'a> {
    /// Fractional coordinates of an atom the document refers to, as
    /// `Derived::fractional` gives; `None` for an atom these atoms do not
    /// have, or no longer name the same way.
    fn fractional(&self, atom: AtomRef) -> Option<[f64; 3]> {
        let atom = match self.ids.map(|ids| ids.atom(atom)) {
            None => atom,
            Some(AtomFate::Kept(atom)) => atom,
            Some(AtomFate::Site(_) | AtomFate::Removed) => return None,
        };
        let orbit = self.orbits.iter().find(|orbit| orbit.site == atom.site)?;
        let position = orbit.positions.get(usize::from(atom.member))?;
        Some([0, 1, 2].map(|i| position[i] + f64::from(atom.cell[i])))
    }
}

/// The document's structure after an edit not yet made, such as an atom
/// being dragged, with what drawing it needs. Its symmetry is not analyzed,
/// which keeps it quick enough to remake on every pointer move.
pub(crate) struct Preview {
    pub structure: Structure,
    /// What the edit would do to references into the document.
    pub ids: IdMap,
    pub orbits: Vec<Orbit>,
    pub displayed: Displayed,
}

impl Preview {
    /// The edited structure as `range` shows it, with the polyhedra of
    /// `polyhedra`'s centers in that model. A sphere around an atom should
    /// be [fixed](DisplayRange::fixed) first, so that the region stays put
    /// while its atom moves.
    pub fn new(
        structure: Structure,
        ids: IdMap,
        range: &DisplayRange,
        polyhedra: Option<&PolyhedronCenters>,
    ) -> Self {
        let orbits = mw_crystal::orbits(&structure);
        let bonds = mw_crystal::bonds(&structure, &orbits, &BondRules::default());
        let displayed = match polyhedra {
            Some(centers) => Displayed::with_polyhedra(&structure, &orbits, &bonds, range, centers),
            None => Displayed::new(&structure, &orbits, &bonds, range),
        };
        Self {
            structure,
            ids,
            orbits,
            displayed,
        }
    }

    pub fn atoms(&self) -> Atoms<'_> {
        Atoms {
            structure: &self.structure,
            orbits: &self.orbits,
            displayed: &self.displayed,
            ids: Some(&self.ids),
        }
    }
}

/// What the viewport draws: one sphere per displayed atom, in the order of
/// `shown.displayed.atoms`, so a picked sphere's index is the atom's index;
/// the bonds between displayed atoms; the coordination polyhedra, if any,
/// translucent in their center's color with darker edges; the edges of the
/// cell at the origin; and a dashed line along each of the document's
/// measurements (`annotations`) whose atoms still exist.
pub(crate) fn draw(
    shown: &Atoms,
    annotations: &[Annotation],
    model: DisplayModel,
    colors: LineColors,
) -> Scene {
    let lattice = &shown.structure.lattice;
    // Looked up once per element rather than once per atom: a range of many
    // cells repeats a few elements thousands of times.
    let mut styles: HashMap<&str, ([f32; 3], f32)> = HashMap::new();
    let spheres: Vec<SphereInstance> = shown
        .displayed
        .atoms
        .iter()
        .map(|atom| {
            let ([r, g, b], radius) = *styles.entry(&atom.element).or_insert_with(|| {
                let (srgb, radius) = element_style(&atom.element);
                (srgb.map(|c| srgb_to_linear(f32::from(c) / 255.0)), radius)
            });
            let [x, y, z] = point(atom.position);
            SphereInstance {
                center_radius: [x, y, z, radius * model.radius_scale()],
                color: [r, g, b, 1.0],
            }
        })
        .collect();

    let bonds = if model.shows_bonds() {
        shown
            .displayed
            .bonds
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

    let atoms = &shown.displayed.atoms;
    let mut polyhedra = Vec::with_capacity(shown.displayed.polyhedra.len());
    for polyhedron in &shown.displayed.polyhedra {
        let [r, g, b, _] = spheres[polyhedron.center].color;
        let corners = polyhedron.vertices();
        // Indices into the corners, which the faces name as atoms.
        let local = |atom: usize| corners.binary_search(&atom).unwrap_or(0) as u32;
        polyhedra.push(Polyhedron {
            vertices: corners.iter().map(|&i| point(atoms[i].position)).collect(),
            triangles: polyhedron.triangles().map(|t| t.map(local)).collect(),
            color: [r, g, b, POLYHEDRON_OPACITY],
        });
        let edge_color = [r, g, b].map(|c| c * POLYHEDRON_EDGE_SHADE);
        let [er, eg, eb] = edge_color;
        lines.extend(polyhedron.edges().into_iter().map(|[i, j]| {
            line(
                atoms[i].position,
                atoms[j].position,
                POLYHEDRON_EDGE_WIDTH,
                0.0,
                [er, eg, eb, 1.0],
            )
        }));
    }
    let measurement_color = linear(colors.measurement);
    // Stale measurements may refer to atoms that are gone or moved.
    for annotation in annotations.iter().filter(|a| !a.stale) {
        let measurement = &annotation.measurement;
        let points: Option<Vec<[f64; 3]>> = measurement
            .atoms()
            .iter()
            .map(|atom| shown.fractional(*atom).map(|p| lattice.to_cartesian(p)))
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
        polyhedra,
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

    fn colors() -> LineColors {
        LineColors {
            cell: gpui_kit::hsla(0.0, 0.0, 0.5, 1.0),
            measurement: gpui_kit::hsla(0.0, 0.0, 0.1, 1.0),
        }
    }

    /// Polyhedra are drawn in their center's color, translucent, with
    /// darker edges; the faces name the polyhedron's own corners.
    #[test]
    fn polyhedra_take_their_centers_color() {
        let sample = mw_crystal::sample::load(mw_crystal::sample::SampleKey::Rutile);
        let displayed = sample.derived.display_polyhedra(
            &sample.structure,
            &DisplayRange::default(),
            &PolyhedronCenters::Default,
        );
        let atoms = Atoms {
            structure: &sample.structure,
            orbits: &sample.derived.orbits,
            displayed: &displayed,
            ids: None,
        };
        let scene = draw(&atoms, &[], DisplayModel::Polyhedra, colors());
        assert_eq!(scene.spheres.len(), displayed.atoms.len());
        assert_eq!(scene.polyhedra.len(), 9);
        // Ti–O bonds are the polyhedra.
        assert!(scene.bonds.is_empty());
        let ti = element_style("Ti")
            .0
            .map(|c| srgb_to_linear(f32::from(c) / 255.0));
        for polyhedron in &scene.polyhedra {
            assert_eq!(polyhedron.color, [ti[0], ti[1], ti[2], POLYHEDRON_OPACITY]);
            assert_eq!(polyhedron.vertices.len(), 6);
            assert_eq!(polyhedron.triangles.len(), 8);
            assert!(polyhedron.triangles.iter().flatten().all(|&i| i < 6));
        }
        // The cell's 12 edges, and 12 per octahedron.
        assert_eq!(scene.lines.len(), 12 + 9 * 12);
        let edge = scene.lines.last().unwrap().color;
        assert!((edge[0] - ti[0] * POLYHEDRON_EDGE_SHADE).abs() < 1e-6);
    }

    /// Viewport design §8.6: making the scene of 10 × 10 × 10 rutile cells
    /// with their 2331 octahedra, on the UI thread.
    #[test]
    fn large_ranges_of_polyhedra_draw_quickly() {
        let sample = mw_crystal::sample::load(mw_crystal::sample::SampleKey::Rutile);
        let range = DisplayRange::Cells {
            counts: [10, 10, 10],
            boundary: true,
        };
        let displayed = sample.derived.display_polyhedra(
            &sample.structure,
            &range,
            &PolyhedronCenters::Default,
        );
        let atoms = Atoms {
            structure: &sample.structure,
            orbits: &sample.derived.orbits,
            displayed: &displayed,
            ids: None,
        };
        let started = std::time::Instant::now();
        let scene = draw(&atoms, &[], DisplayModel::Polyhedra, colors());
        let elapsed = started.elapsed();
        assert_eq!(scene.polyhedra.len(), 2331);
        eprintln!(
            "scene of 10 × 10 × 10 rutile polyhedra: {} spheres, {} polyhedra, {} lines in {elapsed:?}",
            scene.spheres.len(),
            scene.polyhedra.len(),
            scene.lines.len()
        );
        assert!(elapsed.as_secs_f64() < 2.0, "{elapsed:?}");
    }
}
