//! Mock: built-in sample structures. They stand in for file import, and
//! their [`Derived`] data stands in for the symmetry engine.
//!
//! Lattice parameters and coordinates are the commonly cited room-temperature
//! values; densities are computed from them.

use crate::{
    Axis, CellConstraints, CellParam, CellParams, Constraint, CoordinateConstraint, CrystalSystem,
    Derived, DisplayAtom, Lattice, Site, SiteId, SiteSymmetry, SpaceGroup, Structure,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SampleKey {
    Rutile,
    RutileSupercell,
    StrontiumTitanate,
}

pub struct Sample {
    pub structure: Structure,
    pub derived: Derived,
}

pub fn load(key: SampleKey) -> Sample {
    match key {
        SampleKey::Rutile => rutile(),
        SampleKey::RutileSupercell => rutile_supercell(),
        SampleKey::StrontiumTitanate => strontium_titanate(),
    }
}

const RUTILE_A: f64 = 4.594;
const RUTILE_C: f64 = 2.959;
const RUTILE_X: f64 = 0.3048;

/// The two Ti and four O positions of one rutile cell (P4₂/mnm, Ti 2a,
/// O 4f at x, x, 0).
fn rutile_cell_contents() -> [(&'static str, [f64; 3]); 6] {
    let x = RUTILE_X;
    [
        ("Ti", [0.0, 0.0, 0.0]),
        ("Ti", [0.5, 0.5, 0.5]),
        ("O", [x, x, 0.0]),
        ("O", [1.0 - x, 1.0 - x, 0.0]),
        ("O", [0.5 + x, 0.5 - x, 0.5]),
        ("O", [0.5 - x, 0.5 + x, 0.5]),
    ]
}

fn fixed() -> [CoordinateConstraint; 3] {
    [CoordinateConstraint::Fixed; 3]
}

fn rutile() -> Sample {
    let lattice = Lattice::new(CellParams::new(
        RUTILE_A, RUTILE_A, RUTILE_C, 90.0, 90.0, 90.0,
    ));
    let ti = SiteId(1);
    let o = SiteId(2);
    let structure = Structure {
        lattice,
        space_group: SpaceGroup {
            hm_symbol: "P4₂/mnm".into(),
            it_number: 136,
        },
        sites: vec![
            Site::single(ti.0, "Ti1", "Ti", [0.0, 0.0, 0.0]),
            Site::single(o.0, "O1", "O", [RUTILE_X, RUTILE_X, 0.0]),
        ],
    };
    let atoms = with_boundary_images(&rutile_cell_contents())
        .into_iter()
        .map(|(element, frac)| DisplayAtom {
            site: if element == "Ti" { ti } else { o },
            element: element.to_string(),
            position: lattice.to_cartesian(frac),
        })
        .collect();
    let derived = Derived {
        crystal_system: CrystalSystem::Tetragonal,
        cell: tetragonal_constraints(),
        sites: vec![
            (
                ti,
                SiteSymmetry {
                    wyckoff: "2a".into(),
                    multiplicity: 2,
                    coordinate_form: "0, 0, 0".into(),
                    coordinates: fixed(),
                },
            ),
            (
                o,
                SiteSymmetry {
                    wyckoff: "4f".into(),
                    multiplicity: 4,
                    coordinate_form: "x, x, 0".into(),
                    coordinates: [
                        CoordinateConstraint::Free,
                        CoordinateConstraint::Follows(Axis::X),
                        CoordinateConstraint::Fixed,
                    ],
                },
            ),
        ],
        atoms,
        formula: "TiO₂".into(),
        density: density(2.0 * TIO2_MASS, lattice.params().volume()),
    };
    Sample { structure, derived }
}

/// A 2 × 2 × 2 rutile supercell in P1: every atom is its own site.
fn rutile_supercell() -> Sample {
    let lattice = Lattice::new(CellParams::new(
        2.0 * RUTILE_A,
        2.0 * RUTILE_A,
        2.0 * RUTILE_C,
        90.0,
        90.0,
        90.0,
    ));
    let mut sites = Vec::new();
    let mut atoms = Vec::new();
    let mut counts = [0u32; 2];
    for (i, j, k) in (0..2).flat_map(|i| (0..2).flat_map(move |j| (0..2).map(move |k| (i, j, k)))) {
        for (element, frac) in rutile_cell_contents() {
            let position = [
                (frac[0] + i as f64) / 2.0,
                (frac[1] + j as f64) / 2.0,
                (frac[2] + k as f64) / 2.0,
            ];
            let count = &mut counts[usize::from(element == "O")];
            *count += 1;
            let id = SiteId(sites.len() as u32 + 1);
            sites.push(Site::single(
                id.0,
                &format!("{element}{count}"),
                element,
                position,
            ));
            atoms.push(DisplayAtom {
                site: id,
                element: element.to_string(),
                position: lattice.to_cartesian(position),
            });
        }
    }
    let general = SiteSymmetry {
        wyckoff: "1a".into(),
        multiplicity: 1,
        coordinate_form: "x, y, z".into(),
        coordinates: [CoordinateConstraint::Free; 3],
    };
    let derived = Derived {
        crystal_system: CrystalSystem::Triclinic,
        cell: CellConstraints::free(),
        sites: sites.iter().map(|s| (s.id, general.clone())).collect(),
        atoms,
        formula: "TiO₂".into(),
        density: density(16.0 * TIO2_MASS, lattice.params().volume()),
    };
    Sample {
        structure: Structure {
            lattice,
            space_group: SpaceGroup {
                hm_symbol: "P1".into(),
                it_number: 1,
            },
            sites,
        },
        derived,
    }
}

const SRTIO3_A: f64 = 3.905;

/// Cubic perovskite SrTiO₃ (Pm-3m, Sr 1a, Ti 1b, O 3c).
fn strontium_titanate() -> Sample {
    let lattice = Lattice::new(CellParams::new(
        SRTIO3_A, SRTIO3_A, SRTIO3_A, 90.0, 90.0, 90.0,
    ));
    let (sr, ti, o) = (SiteId(1), SiteId(2), SiteId(3));
    let contents = [
        ("Sr", [0.0, 0.0, 0.0]),
        ("Ti", [0.5, 0.5, 0.5]),
        ("O", [0.0, 0.5, 0.5]),
        ("O", [0.5, 0.0, 0.5]),
        ("O", [0.5, 0.5, 0.0]),
    ];
    let atoms = with_boundary_images(&contents)
        .into_iter()
        .map(|(element, frac)| DisplayAtom {
            site: match element {
                "Sr" => sr,
                "Ti" => ti,
                _ => o,
            },
            element: element.to_string(),
            position: lattice.to_cartesian(frac),
        })
        .collect();
    let symmetry = |wyckoff: &str, multiplicity, form: &str| SiteSymmetry {
        wyckoff: wyckoff.into(),
        multiplicity,
        coordinate_form: form.into(),
        coordinates: fixed(),
    };
    let derived = Derived {
        crystal_system: CrystalSystem::Cubic,
        cell: CellConstraints::new([
            Constraint::Free,
            Constraint::Equals(CellParam::A),
            Constraint::Equals(CellParam::A),
            Constraint::Fixed,
            Constraint::Fixed,
            Constraint::Fixed,
        ]),
        sites: vec![
            (sr, symmetry("1a", 1, "0, 0, 0")),
            (ti, symmetry("1b", 1, "½, ½, ½")),
            (o, symmetry("3c", 3, "0, ½, ½")),
        ],
        atoms,
        formula: "SrTiO₃".into(),
        density: density(SRTIO3_MASS, lattice.params().volume()),
    };
    Sample {
        structure: Structure {
            lattice,
            space_group: SpaceGroup {
                hm_symbol: "Pm-3m".into(),
                it_number: 221,
            },
            sites: vec![
                Site::single(sr.0, "Sr1", "Sr", [0.0, 0.0, 0.0]),
                Site::single(ti.0, "Ti1", "Ti", [0.5, 0.5, 0.5]),
                Site::single(o.0, "O1", "O", [0.0, 0.5, 0.5]),
            ],
        },
        derived,
    }
}

fn tetragonal_constraints() -> CellConstraints {
    CellConstraints::new([
        Constraint::Free,
        Constraint::Equals(CellParam::A),
        Constraint::Free,
        Constraint::Fixed,
        Constraint::Fixed,
        Constraint::Fixed,
    ])
}

/// Repeats every atom lying on a cell face at the opposite face, the way a
/// 0..=1 display range shows a unit cell. A stand-in for the view layer's
/// display range (design §5.3).
fn with_boundary_images<'a>(contents: &[(&'a str, [f64; 3])]) -> Vec<(&'a str, [f64; 3])> {
    let mut atoms = Vec::new();
    for &(element, frac) in contents {
        let shifts = |v: f64| {
            if v.abs() < 1e-9 {
                vec![0.0, 1.0]
            } else {
                vec![0.0]
            }
        };
        for dx in shifts(frac[0]) {
            for dy in shifts(frac[1]) {
                for dz in shifts(frac[2]) {
                    atoms.push((element, [frac[0] + dx, frac[1] + dy, frac[2] + dz]));
                }
            }
        }
    }
    atoms
}

const AVOGADRO: f64 = 6.022_140_76e23;
const TIO2_MASS: f64 = 47.867 + 2.0 * 15.999;
const SRTIO3_MASS: f64 = 87.62 + 47.867 + 3.0 * 15.999;

/// Density in g/cm³ of `mass` g/mol of atoms in `volume` Å³.
fn density(mass: f64, volume: f64) -> f64 {
    mass / (AVOGADRO * volume * 1e-24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rutile_shows_a_full_cell() {
        let sample = load(SampleKey::Rutile);
        // 8 corner Ti + 1 body-center Ti + 4 O, two of them repeated at z = 1.
        assert_eq!(sample.derived.atoms.len(), 15);
        assert!((sample.derived.density - 4.25).abs() < 0.01);
    }

    #[test]
    fn strontium_titanate_shows_a_full_cell() {
        let sample = load(SampleKey::StrontiumTitanate);
        assert_eq!(sample.derived.atoms.len(), 8 + 1 + 6);
        assert!((sample.derived.density - 5.12).abs() < 0.01);
    }

    #[test]
    fn supercell_is_p1_with_one_site_per_atom() {
        let sample = load(SampleKey::RutileSupercell);
        assert_eq!(sample.structure.sites.len(), 48);
        assert_eq!(sample.derived.atoms.len(), 48);
        assert_eq!(sample.structure.edit_mode(), crate::EditMode::P1);
    }

    #[test]
    fn every_display_atom_and_symmetry_refers_to_a_site() {
        for key in [
            SampleKey::Rutile,
            SampleKey::RutileSupercell,
            SampleKey::StrontiumTitanate,
        ] {
            let sample = load(key);
            for atom in &sample.derived.atoms {
                assert!(sample.structure.site(atom.site).is_some(), "{key:?}");
            }
            for site in &sample.structure.sites {
                assert!(sample.derived.site(site.id).is_some(), "{key:?}");
            }
        }
    }
}
