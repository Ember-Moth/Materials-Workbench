//! Bonds (design §5.3): pairs of atoms closer than the sum of their covalent
//! radii plus a tolerance, found across periodic boundaries with a cell list
//! in O(N).
//!
//! Each bond of the crystal is stored once per unit cell, as a pair of
//! [`AtomRef`]s whose first atom lies in the cell at the origin. The bonds
//! drawn are those between displayed atoms ([`display_bonds`]).

use std::collections::HashMap;

use crate::{AtomRef, DisplayAtom, Orbit, Site, SiteId, Structure, element};

/// Single-bond covalent radii in Å by atomic number − 1, H to Cm (Cordero et
/// al., Dalton Trans. 2008, 2832; low-spin values for Mn, Fe and Co, sp³ for
/// C). Heavier elements have no tabulated radius.
const COVALENT_RADII: [f64; 96] = [
    0.31, 0.28, // H, He
    1.28, 0.96, 0.84, 0.76, 0.71, 0.66, 0.57, 0.58, // Li–Ne
    1.66, 1.41, 1.21, 1.11, 1.07, 1.05, 1.02, 1.06, // Na–Ar
    2.03, 1.76, 1.70, 1.60, 1.53, 1.39, 1.39, 1.32, 1.26, // K–Co
    1.24, 1.32, 1.22, 1.22, 1.20, 1.19, 1.20, 1.20, 1.16, // Ni–Kr
    2.20, 1.95, 1.90, 1.75, 1.64, 1.54, 1.47, 1.46, 1.42, // Rb–Rh
    1.39, 1.45, 1.44, 1.42, 1.39, 1.39, 1.38, 1.39, 1.40, // Pd–Xe
    2.44, 2.15, 2.07, 2.04, 2.03, 2.01, 1.99, 1.98, 1.98, // Cs–Eu
    1.96, 1.94, 1.92, 1.92, 1.89, 1.90, 1.87, 1.87, // Gd–Lu
    1.75, 1.70, 1.62, 1.51, 1.44, 1.41, 1.36, 1.36, 1.32, // Hf–Hg
    1.45, 1.46, 1.48, 1.40, 1.50, 1.50, // Tl–Rn
    2.60, 2.21, 2.15, 2.06, 2.00, 1.96, 1.90, 1.87, 1.80, 1.69, // Fr–Cm
];

/// Elements that are not metals for bonding purposes: the nonmetals, noble
/// gases and metalloids.
const NONMETALS: [&str; 24] = [
    "H", "He", "B", "C", "N", "O", "F", "Ne", "Si", "P", "S", "Cl", "Ar", "Ge", "As", "Se", "Br",
    "Kr", "Sb", "Te", "I", "Xe", "At", "Rn",
];

/// Covalent radius in Å, if one is tabulated for the element.
pub fn covalent_radius(symbol: &str) -> Option<f64> {
    let number = element::atomic_number(symbol)?;
    COVALENT_RADII.get(usize::from(number) - 1).copied()
}

fn is_metal(symbol: &str) -> bool {
    !NONMETALS.contains(&symbol)
}

/// When two atoms count as bonded (design §6, the document's analysis
/// settings). Per-pair cutoffs and user overrides come with editing.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct BondRules {
    /// Å added to the sum of the covalent radii.
    pub tolerance: f64,
    /// Å. Closer atoms are not bonded; they are disorder partners or errors,
    /// reported as close contacts instead.
    pub min_length: f64,
    /// Whether two metal atoms can bond. Off by default: in ionic crystals
    /// the cations are often closer than their covalent radii add up to (Ti–Ti
    /// in rutile is 2.96 Å against 3.20 Å), and drawing those bonds hides the
    /// coordination polyhedra.
    pub metal_pairs: bool,
}

impl Default for BondRules {
    fn default() -> Self {
        // Jmol's and Open Babel's defaults.
        Self {
            tolerance: 0.45,
            min_length: 0.4,
            metal_pairs: false,
        }
    }
}

/// A bond between two atoms of the crystal. `a` is in the cell at the
/// origin; `b` may be in a neighboring cell.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Bond {
    pub a: AtomRef,
    pub b: AtomRef,
    /// Å.
    pub length: f64,
}

/// An atom of the unit cell that can bond.
struct Bondable<'a> {
    atom: AtomRef,
    site: &'a Site,
    radius: f64,
    metal: bool,
    cartesian: [f64; 3],
    fractional: [f64; 3],
}

/// A periodic image of a bondable atom near the unit cell.
struct Image {
    index: usize,
    cell: [i32; 3],
    cartesian: [f64; 3],
}

fn to_cartesian(m: &[[f64; 3]; 3], f: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| f[0] * m[0][i] + f[1] * m[1][i] + f[2] * m[2][i])
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    norm([a[0] - b[0], a[1] - b[1], a[2] - b[2]])
}

/// Split positions of one disorder assembly exclude each other: atoms of
/// different groups are alternatives, not neighbors. Group 0 is the ordered
/// part, as in SHELX.
fn alternatives(a: &Site, b: &Site) -> bool {
    match (&a.disorder, &b.disorder) {
        (Some(a), Some(b)) => {
            a.assembly == b.assembly && a.group != b.group && a.group != "0" && b.group != "0"
        }
        _ => false,
    }
}

/// Every bond of the crystal, once per unit cell, ordered by first atom.
pub fn bonds(structure: &Structure, orbits: &[Orbit], rules: &BondRules) -> Vec<Bond> {
    let params = structure.lattice.params();
    let matrix = params.matrix();
    let atoms: Vec<Bondable> = orbits
        .iter()
        .filter_map(|orbit| Some((orbit, structure.site(orbit.site)?)))
        .flat_map(|(orbit, site)| {
            let radius = covalent_radius(site.element());
            orbit
                .positions
                .iter()
                .enumerate()
                .filter_map(move |(member, &fractional)| {
                    Some(Bondable {
                        atom: AtomRef::new(orbit.site, member as u16, [0; 3]),
                        site,
                        radius: radius?,
                        metal: is_metal(site.element()),
                        cartesian: to_cartesian(&matrix, fractional),
                        fractional,
                    })
                })
        })
        .collect();
    let Some(max_radius) = atoms.iter().map(|a| a.radius).max_by(f64::total_cmp) else {
        return Vec::new();
    };
    let cutoff = 2.0 * max_radius + rules.tolerance;
    let volume = params.volume();
    // A degenerate cell has no lattice planes to bound the search by.
    if !volume.is_finite() || volume <= 0.0 {
        return Vec::new();
    }

    // A neighbor within the cutoff of an atom in the cell lies at most
    // cutoff / spacing cells beyond it along each axis, where spacing is the
    // distance between lattice planes.
    let reach =
        [(1, 2), (2, 0), (0, 1)].map(|(i, j)| cutoff * norm(cross(matrix[i], matrix[j])) / volume);
    let images: Vec<Image> = atoms
        .iter()
        .enumerate()
        .flat_map(|(index, atom)| {
            let range = |axis: usize| {
                let low = (-reach[axis] - atom.fractional[axis]).floor() as i32;
                let high = (1.0 + reach[axis] - atom.fractional[axis]).ceil() as i32;
                low..=high
            };
            let mut images = Vec::new();
            for x in range(0) {
                for y in range(1) {
                    for z in range(2) {
                        let cell = [x, y, z];
                        let f = [0, 1, 2].map(|i| atom.fractional[i] + f64::from(cell[i]));
                        if (0..3).all(|i| f[i] >= -reach[i] && f[i] <= 1.0 + reach[i]) {
                            images.push(Image {
                                index,
                                cell,
                                cartesian: to_cartesian(&matrix, f),
                            });
                        }
                    }
                }
            }
            images
        })
        .collect();

    let bin = |p: [f64; 3]| p.map(|v| (v / cutoff).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (i, image) in images.iter().enumerate() {
        grid.entry(bin(image.cartesian)).or_default().push(i);
    }

    let mut bonds = Vec::new();
    for (i, a) in atoms.iter().enumerate() {
        let home = bin(a.cartesian);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(bucket) = grid.get(&[home[0] + dx, home[1] + dy, home[2] + dz]) else {
                        continue;
                    };
                    for image in bucket.iter().map(|&k| &images[k]) {
                        // Each bond is found from both ends; keep one.
                        let j = image.index;
                        if j < i || (j == i && image.cell <= [0; 3]) {
                            continue;
                        }
                        let b = &atoms[j];
                        let length = distance(a.cartesian, image.cartesian);
                        if length < rules.min_length
                            || length > a.radius + b.radius + rules.tolerance
                            || (a.metal && b.metal && !rules.metal_pairs)
                            || alternatives(a.site, b.site)
                        {
                            continue;
                        }
                        bonds.push(Bond {
                            a: a.atom,
                            b: b.atom.translated(image.cell),
                            length,
                        });
                    }
                }
            }
        }
    }
    bonds
}

/// The bonds between displayed atoms, as index pairs into `atoms`.
pub fn display_bonds(atoms: &[DisplayAtom], bonds: &[Bond]) -> Vec<[usize; 2]> {
    let index: HashMap<AtomRef, usize> = atoms
        .iter()
        .enumerate()
        .map(|(i, atom)| (atom.atom, i))
        .collect();
    let mut from: HashMap<(SiteId, u16), Vec<&Bond>> = HashMap::new();
    for bond in bonds {
        from.entry((bond.a.site, bond.a.member))
            .or_default()
            .push(bond);
    }
    let mut pairs = Vec::new();
    for (i, atom) in atoms.iter().enumerate() {
        let start = atom.atom;
        for bond in from.get(&(start.site, start.member)).into_iter().flatten() {
            if let Some(&j) = index.get(&bond.b.translated(start.cell)) {
                pairs.push([i, j]);
            }
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{self, SampleKey};
    use crate::{CellParams, Disorder, Lattice, Symmetry, expand};

    fn p1(cell: f64, sites: Vec<Site>) -> Structure {
        Structure::new(
            Lattice::new(CellParams::new(cell, cell, cell, 90.0, 90.0, 90.0)),
            Symmetry::p1(),
            sites,
        )
    }

    fn crystal_bonds(structure: &Structure) -> Vec<Bond> {
        bonds(structure, &expand::orbits(structure), &BondRules::default())
    }

    #[test]
    fn radii_cover_the_table() {
        assert_eq!(covalent_radius("H"), Some(0.31));
        assert_eq!(covalent_radius("O"), Some(0.66));
        assert_eq!(covalent_radius("Cm"), Some(1.69));
        assert_eq!(covalent_radius("Bk"), None);
        assert_eq!(covalent_radius(crate::UNKNOWN_ELEMENT), None);
        for symbol in NONMETALS {
            assert!(element::is_element(symbol), "{symbol}");
        }
    }

    #[test]
    fn rutile_titanium_is_octahedral() {
        let sample = sample::load(SampleKey::Rutile);
        let bonds = &sample.derived.bonds;
        // Two Ti per cell, six Ti–O bonds each; no Ti–Ti and no O–O.
        assert_eq!(bonds.len(), 12);
        let ti = sample.structure.sites[0].id;
        for bond in bonds {
            assert_ne!(bond.a.site == ti, bond.b.site == ti, "{bond:?}");
            assert_eq!(bond.a.cell, [0; 3]);
        }
        let mut lengths: Vec<f64> = bonds.iter().map(|b| b.length).collect();
        lengths.sort_by(f64::total_cmp);
        assert!((lengths[0] - 1.948).abs() < 1e-3, "{lengths:?}");
        assert!((lengths[11] - 1.980).abs() < 1e-3, "{lengths:?}");
    }

    #[test]
    fn bonds_cross_cell_boundaries() {
        // In a 4 Å cell, C at 0 is 2.8 Å from O at 0.7, too far to bond, but
        // 1.2 Å from that O's image at −0.3 in the cell to the left.
        let structure = p1(
            4.0,
            vec![
                Site::single(1, "C1", "C", [0.0, 0.0, 0.0]),
                Site::single(2, "O1", "O", [0.7, 0.0, 0.0]),
            ],
        );
        let bonds = crystal_bonds(&structure);
        assert_eq!(bonds.len(), 1, "{bonds:?}");
        assert_eq!(bonds[0].a, AtomRef::new(SiteId(1), 0, [0; 3]));
        assert_eq!(bonds[0].b, AtomRef::new(SiteId(2), 0, [-1, 0, 0]));
        assert!((bonds[0].length - 1.2).abs() < 1e-9);
    }

    #[test]
    fn an_atom_bonds_to_its_own_images_once() {
        // A carbon chain: each atom bonds to the next cell's copy of itself.
        let structure = Structure::new(
            Lattice::new(CellParams::new(1.5, 10.0, 10.0, 90.0, 90.0, 90.0)),
            Symmetry::p1(),
            vec![Site::single(1, "C1", "C", [0.0, 0.5, 0.5])],
        );
        let bonds = crystal_bonds(&structure);
        assert_eq!(bonds.len(), 1, "{bonds:?}");
        assert_eq!(bonds[0].b.cell, [1, 0, 0]);
    }

    #[test]
    fn skew_cells_find_every_neighbor() {
        // Diamond silicon in its primitive (rhombohedral, 60°) cell: four
        // Si–Si bonds per atom, two atoms, so four bonds per cell.
        let a = 5.431 / 2f64.sqrt();
        let structure = Structure::new(
            Lattice::new(CellParams::new(a, a, a, 60.0, 60.0, 60.0)),
            Symmetry::p1(),
            vec![
                Site::single(1, "Si1", "Si", [0.0, 0.0, 0.0]),
                Site::single(2, "Si2", "Si", [0.25, 0.25, 0.25]),
            ],
        );
        let bonds = crystal_bonds(&structure);
        assert_eq!(bonds.len(), 4, "{bonds:?}");
        for bond in &bonds {
            assert!((bond.length - 5.431 * 3f64.sqrt() / 4.0).abs() < 1e-9);
        }
    }

    #[test]
    fn metals_and_disorder_partners_do_not_bond() {
        let structure = p1(
            10.0,
            vec![
                Site::single(1, "Na1", "Na", [0.0, 0.0, 0.0]),
                Site::single(2, "Na2", "Na", [0.25, 0.0, 0.0]),
                Site::single(3, "O1", "O", [0.5, 0.5, 0.5])
                    .with_disorder(Disorder::new(Some("A".into()), "1".into())),
                Site::single(4, "O2", "O", [0.6, 0.5, 0.5])
                    .with_disorder(Disorder::new(Some("A".into()), "2".into())),
            ],
        );
        assert!(crystal_bonds(&structure).is_empty());

        let rules = BondRules {
            metal_pairs: true,
            ..BondRules::default()
        };
        let bonds = bonds(&structure, &expand::orbits(&structure), &rules);
        assert_eq!(bonds.len(), 1);
        assert_eq!((bonds[0].a.site, bonds[0].b.site), (SiteId(1), SiteId(2)));
    }

    #[test]
    fn displayed_bonds_join_displayed_atoms() {
        let sample = sample::load(SampleKey::StrontiumTitanate);
        let derived = &sample.derived;
        // The body-center Ti bonds to the six face-center O. Sr–O bonds run
        // from each face center to the four corners of its face.
        assert_eq!(derived.display_bonds.len(), 6 + 6 * 4);
        for [i, j] in &derived.display_bonds {
            let (a, b) = (&derived.atoms[*i], &derived.atoms[*j]);
            let length = crate::atom_ref::distance(a.position, b.position);
            assert!(length < 2.8, "{a:?} {b:?}");
        }
    }

    #[test]
    fn large_cells_stay_fast() {
        // 8000 atoms; a quadratic search would take seconds in debug builds.
        let n = 20;
        let mut sites = Vec::new();
        for i in 0..n {
            for j in 0..n {
                for k in 0..n {
                    let id = sites.len() as u32 + 1;
                    let element = if (i + j + k) % 2 == 0 { "Na" } else { "Cl" };
                    let f = |v: usize| v as f64 / n as f64;
                    sites.push(Site::single(id, element, element, [f(i), f(j), f(k)]));
                }
            }
        }
        let structure = p1(2.82 * n as f64, sites);
        let started = std::time::Instant::now();
        let bonds = crystal_bonds(&structure);
        // Rock salt: six neighbors per atom, each bond counted once.
        assert_eq!(bonds.len(), n * n * n * 3);
        assert!(
            started.elapsed().as_secs_f64() < 2.0,
            "{:?}",
            started.elapsed()
        );
    }
}
