//! Space group types (design §4.4): identifying the type of an operation
//! set, operation sets from space group names, Wyckoff positions, and the
//! symmetry a structure's atoms actually have (design §7.10).

use std::fmt;

use crate::expand::Orbit;
use crate::lattice::norm;
use crate::tables::{self, Transform};
use crate::{CellParams, CrystalSystem, SiteId, Structure, Symmetry, constraints, linalg};

/// How an operation set relates to the tabulated settings of its type.
#[derive(Clone, Debug, PartialEq)]
pub enum Setting {
    /// The setting the International Tables list first: unique axis b,
    /// cell choice 1, origin choice 2, hexagonal axes.
    Standard,
    /// Another tabulated setting, e.g. `P 1 1 2_1/a` with code `c1`.
    Tabulated {
        /// Full Hermann–Mauguin symbol of the setting.
        symbol: &'static str,
        /// Unique axis and cell choice, axes, or origin choice, e.g. `c1`,
        /// `ba-c`, `1`, `R`.
        code: &'static str,
    },
    /// No tabulated setting; a change of basis or origin relates the
    /// operations to the standard setting, in which Wyckoff positions are
    /// then given.
    Transformed,
}

/// The space group type of an operation set. The operation set is the
/// source of truth; this names it (design §4.4).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SpaceGroupType {
    /// International Tables number, 1–230.
    pub number: u16,
    /// Hall number (1–530) of the tabulated setting the operations match,
    /// or of the standard setting for [`Setting::Transformed`].
    pub hall_number: u16,
    /// Short Hermann–Mauguin symbol of the type, e.g. `P 4_2/m n m`.
    pub hm_symbol: &'static str,
    pub setting: Setting,
    /// From the operations' coordinates to those of the tabulated setting;
    /// `None` when the operations are that setting.
    to_table: Option<Transform>,
}

impl SpaceGroupType {
    /// Identifies the type of an operation set: exactly one of the 530
    /// tabulated settings, or failing that, any setting by moyo's search.
    /// `params` only needs to be close to a cell the operations preserve.
    pub fn identify(symmetry: &Symmetry, params: &CellParams) -> Option<Self> {
        if let Some(setting) = tables::match_operations(symmetry) {
            return Some(Self {
                number: setting.number,
                hall_number: setting.hall_number,
                hm_symbol: setting.hm_short,
                setting: if setting.is_standard() {
                    Setting::Standard
                } else {
                    Setting::Tabulated {
                        symbol: setting.hm_full,
                        code: setting.code,
                    }
                },
                to_table: None,
            });
        }
        let metric = constraints::symmetrized_metric(symmetry, params.metric());
        let (setting, transform) = tables::search_operations(symmetry, metric)?;
        Some(Self {
            number: setting.number,
            hall_number: setting.hall_number,
            hm_symbol: setting.hm_short,
            setting: Setting::Transformed,
            to_table: Some(transform),
        })
    }

    pub fn crystal_system(&self) -> CrystalSystem {
        CrystalSystem::of(self.number)
    }

    /// The Hall symbol of the operations, e.g. `-P 4n 2n`; `None` for
    /// [`Setting::Transformed`], whose operations no tabulated symbol names.
    pub fn hall_symbol(&self) -> Option<&'static str> {
        self.to_table
            .is_none()
            .then(|| tables::setting(self.hall_number).hall_symbol)
    }

    /// The Wyckoff position of a point whose site-symmetry group has
    /// `order` operations. Among the tabulated positions with the matching
    /// multiplicity, the one whose coordinate triplet holds an image of the
    /// point within `tolerance` Å.
    pub(crate) fn wyckoff(
        &self,
        params: &CellParams,
        position: [f64; 3],
        order: usize,
        tolerance: f64,
    ) -> Option<Wyckoff> {
        let setting = tables::setting(self.hall_number);
        let operations = setting.symmetry.operations();
        if order == 0 || !operations.len().is_multiple_of(order) {
            return None;
        }
        let multiplicity = operations.len() / order;
        let candidates: Vec<_> = tables::wyckoff_positions(self.hall_number)
            .filter(|w| w.multiplicity == multiplicity)
            .collect();
        let found = if let [only] = &candidates[..] {
            // Every point lies on exactly one Wyckoff position.
            Some(only)
        } else {
            let mut metric = params.metric();
            let mut y = position;
            if let Some(transform) = &self.to_table {
                y = transform.apply(position);
                metric = transform_metric(&metric, &transform.linear);
            }
            let lines: Vec<_> = candidates
                .iter()
                .map(|c| (c, Line::new(&c.coordinates)))
                .collect();
            operations.iter().find_map(|op| {
                let image = op.apply(y);
                lines
                    .iter()
                    .find(|(_, line)| line.distance(&metric, image) < tolerance)
                    .map(|(c, _)| *c)
            })
        };
        found.map(|w| Wyckoff {
            multiplicity: w.multiplicity,
            letter: w.letter,
            site_symmetry: w.site_symmetry,
        })
    }
}

/// `Pᵀ·G·P`: the metric in the basis `A·P`.
fn transform_metric(metric: &[[f64; 3]; 3], p: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    [0, 1, 2].map(|i| {
        [0, 1, 2].map(|j| {
            let mut sum = 0.0;
            for a in 0..3 {
                for b in 0..3 {
                    sum += p[a][i] * metric[a][b] * p[b][j];
                }
            }
            sum
        })
    })
}

/// The points of a tabulated coordinate triplet such as `x,2x,1/4`: its
/// constant plus the span of its free directions, row-reduced.
struct Line {
    origin: [f64; 3],
    /// Basis rows; `basis[k][pivots[k]] = 1`, and 0 at the other pivots.
    basis: Vec<[f64; 3]>,
    pivots: Vec<usize>,
}

impl Line {
    fn new(coordinates: &crate::SymOp) -> Self {
        let w = coordinates.rotation();
        let mut basis: Vec<[crate::Rational; 3]> = (0..3)
            .map(|j| [0, 1, 2].map(|i| crate::Rational::integer(i64::from(w[i][j]))))
            .collect();
        let pivots = linalg::rref(&mut basis);
        Self {
            origin: coordinates.translation().map(crate::Rational::to_f64),
            basis: basis
                .iter()
                .map(|row| row.map(crate::Rational::to_f64))
                .collect(),
            pivots,
        }
    }

    /// Å from `point` to the nearest lattice image of these points. Free
    /// coordinates take the point's own values, shifted by up to one cell
    /// in case a fractional coefficient makes the shift matter.
    fn distance(&self, metric: &[[f64; 3]; 3], point: [f64; 3]) -> f64 {
        let r = [0, 1, 2].map(|i| point[i] - self.origin[i]);
        let shifts = 3_usize.pow(self.pivots.len() as u32);
        (0..shifts)
            .map(|code| {
                let mut offset = [0.0; 3];
                let mut code = code;
                for (row, &pivot) in self.basis.iter().zip(&self.pivots) {
                    let parameter = r[pivot] + (code % 3) as f64 - 1.0;
                    code /= 3;
                    for i in 0..3 {
                        offset[i] += row[i] * parameter;
                    }
                }
                let residual = [0, 1, 2].map(|i| {
                    let d = r[i] - offset[i];
                    d - d.round()
                });
                norm(metric, residual)
            })
            .fold(f64::INFINITY, f64::min)
    }
}

/// A Wyckoff position as the International Tables label it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Wyckoff {
    /// In the conventional cell of the tabulated setting, which can differ
    /// from the number of atoms in a cell of another setting.
    pub multiplicity: usize,
    pub letter: char,
    /// Oriented site symmetry symbol, e.g. `m.2m`.
    pub site_symmetry: &'static str,
}

impl fmt::Display for Wyckoff {
    /// `4f`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.multiplicity, self.letter)
    }
}

impl Symmetry {
    /// The operations a Hall symbol such as `-P 2ybc` generates.
    pub fn from_hall_symbol(symbol: &str) -> Option<Self> {
        tables::hall_operations(symbol)
    }

    /// The operations of the setting a Hermann–Mauguin symbol names, e.g.
    /// `P 1 21/c 1`, `F d -3 m :1`. A short symbol, or one without an
    /// origin choice, means the standard setting.
    pub fn from_hm_symbol(symbol: &str) -> Option<Self> {
        Some(tables::find_hm(symbol)?.symmetry.clone())
    }

    /// The operations of a type in its standard setting.
    pub fn from_it_number(number: u16) -> Option<Self> {
        (1..=230)
            .contains(&number)
            .then(|| tables::standard(number).symmetry.clone())
    }
}

/// The International Tables number a Hermann–Mauguin symbol names.
pub fn it_number_of(hm_symbol: &str) -> Option<u16> {
    Some(tables::find_hm(hm_symbol)?.number)
}

/// The symmetry a structure's atoms have, which can be higher than its
/// operation set, e.g. a cubic perovskite written in P1 (design §7.10).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct DetectedSymmetry {
    pub number: u16,
    /// Short Hermann–Mauguin symbol, e.g. `P m -3 m`.
    pub hm_symbol: &'static str,
    /// Operations found in the structure's cell.
    pub operations: usize,
    /// Each site's Wyckoff letter in the standard setting of the detected
    /// type.
    pub wyckoffs: Vec<(SiteId, char)>,
}

/// Searches the symmetry of the atoms, telling apart sites by their
/// occupants and disorder groups, with the structure's `symprec`.
pub fn detect_symmetry(structure: &Structure, orbits: &[Orbit]) -> Option<DetectedSymmetry> {
    // A left-handed lattice holds the mirror image of what its coordinates
    // describe on a right-handed basis: searching at −x tells apart
    // enantiomorphic types such as P3₁21 and P3₂21.
    let sign = if structure.lattice.is_left_handed() {
        -1.0
    } else {
        1.0
    };
    let mut kinds: Vec<String> = Vec::new();
    let mut positions = Vec::new();
    let mut species = Vec::new();
    let mut first_atoms = Vec::new();
    for orbit in orbits {
        let site = structure.site(orbit.site)?;
        let kind = crate::symmetrize::kind_key(site);
        let number = match kinds.iter().position(|k| *k == kind) {
            Some(i) => i,
            None => {
                kinds.push(kind);
                kinds.len() - 1
            }
        };
        first_atoms.push((site.id, positions.len()));
        for position in &orbit.positions {
            positions.push(position.map(|v| sign * v));
            species.push(number as i32);
        }
    }
    if positions.is_empty() {
        return None;
    }
    // The search needs only the metric and the fractional positions; the
    // standard orientation hands moyo a right-handed basis whatever the
    // lattice's own orientation.
    let search = tables::search_atoms(
        structure.lattice.params().standard_matrix(),
        &positions,
        &species,
        structure.tolerances.symprec,
    )?;
    Some(DetectedSymmetry {
        number: search.setting.number,
        hm_symbol: search.setting.hm_short,
        operations: search.operations,
        wyckoffs: first_atoms
            .iter()
            .map(|(id, atom)| (*id, search.wyckoffs[*atom]))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SymOp;

    fn group(generators: &[&str]) -> Symmetry {
        let generators: Vec<SymOp> = generators
            .iter()
            .map(|g| SymOp::parse(g).unwrap())
            .collect();
        Symmetry::generate(&generators).unwrap()
    }

    fn cubic() -> CellParams {
        CellParams::new(4.0, 4.0, 4.0, 90.0, 90.0, 90.0)
    }

    fn monoclinic() -> CellParams {
        CellParams::new(5.0, 6.0, 7.0, 90.0, 100.0, 90.0)
    }

    #[test]
    fn tabulated_settings_are_matched_exactly() {
        let rutile = group(&["-y+1/2,x+1/2,z+1/2", "-x+1/2,y+1/2,-z+1/2", "-x,-y,-z"]);
        let params = CellParams::new(4.594, 4.594, 2.959, 90.0, 90.0, 90.0);
        let sg = SpaceGroupType::identify(&rutile, &params).unwrap();
        assert_eq!(
            (sg.number, sg.hall_number, sg.hm_symbol),
            (136, 419, "P 4_2/m n m")
        );
        assert_eq!(sg.setting, Setting::Standard);
        assert_eq!(sg.crystal_system(), CrystalSystem::Tetragonal);
        assert_eq!(sg.hall_symbol(), Some("-P 4n 2n"));

        let p21a = group(&["-x+1/2,-y,z+1/2", "-x,-y,-z"]);
        let sg = SpaceGroupType::identify(&p21a, &monoclinic()).unwrap();
        assert_eq!(sg.number, 14);
        assert!(
            matches!(sg.setting, Setting::Tabulated { code: "c1", .. }),
            "{sg:?}"
        );
    }

    #[test]
    fn other_settings_are_found_by_search() {
        // P2₁/c with the inversion center at 1/4, 1/4, 1/4.
        let shifted = group(&["-x,y+1/2,-z+1/2", "-x+1/2,-y+1/2,-z+1/2"]);
        let sg = SpaceGroupType::identify(&shifted, &monoclinic()).unwrap();
        assert_eq!((sg.number, sg.setting.clone()), (14, Setting::Transformed));
        assert_eq!(sg.hall_symbol(), None);

        // I4/mmm in a face-centered cell twice the size.
        let f4mmm = group(&[
            "-y,x,z",
            "x,-y,-z",
            "-x,-y,-z",
            "x+1/2,y+1/2,z",
            "x+1/2,y,z+1/2",
        ]);
        let params = CellParams::new(5.0, 5.0, 4.0, 90.0, 90.0, 90.0);
        let sg = SpaceGroupType::identify(&f4mmm, &params).unwrap();
        assert_eq!((sg.number, sg.hm_symbol), (139, "I 4/m m m"));
        // 0, 0, 0 is 2a of I4/mmm; 1/2, 0, 0 is 2b (0, 0, 1/2 there).
        let wyckoff = |x| sg.wyckoff(&params, x, 16, 0.01).map(|w| w.to_string());
        assert_eq!(wyckoff([0.0, 0.0, 0.0]).as_deref(), Some("2a"));
        assert_eq!(wyckoff([0.0, 0.0, 0.5]).as_deref(), Some("2b"));
    }

    #[test]
    fn left_handed_lattices_hold_the_other_enantiomorph() {
        // Atoms of P3₁21 written out in P1.
        let hexagonal = CellParams::new(4.9, 4.9, 5.4, 90.0, 90.0, 120.0);
        let structure = Structure::new(
            crate::Lattice::new(hexagonal),
            Symmetry::from_hm_symbol("P 31 2 1").unwrap(),
            vec![
                crate::Site::single(1, "Si1", "Si", [0.47, 0.0, 1.0 / 3.0]),
                crate::Site::single(2, "O1", "O", [0.41, 0.27, 0.12]),
            ],
        );
        let p1 = crate::transform::expand_to_p1(&structure).structure;
        let detected = |structure: &Structure| {
            detect_symmetry(structure, &crate::orbits(structure))
                .unwrap()
                .number
        };
        assert_eq!(detected(&p1), 152);
        // The same coordinates on a left-handed basis: the mirror image.
        let mirror = crate::Rotation::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]]);
        let mut mirrored = p1.clone();
        mirrored.lattice =
            crate::Lattice::with_orientation(hexagonal, crate::Orientation::new(mirror.unwrap()));
        assert_eq!(detected(&mirrored), 154);
        assert_eq!(detected(&crate::right_handed(&mirrored)), 154);
    }

    #[test]
    fn reverse_rhombohedral_setting() {
        // Centering (1/3, 2/3, 1/3); the tables list the obverse setting.
        let reverse = group(&["-y,x-y,z", "y,x,-z", "-x,-y,-z", "x+1/3,y+2/3,z+1/3"]);
        let params = CellParams::new(3.0, 3.0, 15.0, 90.0, 90.0, 120.0);
        let sg = SpaceGroupType::identify(&reverse, &params).unwrap();
        assert_eq!((sg.number, sg.setting.clone()), (166, Setting::Transformed));
        assert_eq!(sg.crystal_system(), CrystalSystem::Trigonal);
    }

    #[test]
    fn wyckoff_letters_in_pm3m() {
        let symmetry = Symmetry::from_hm_symbol("P m -3 m").unwrap();
        let sg = SpaceGroupType::identify(&symmetry, &cubic()).unwrap();
        let wyckoff = |x, order| sg.wyckoff(&cubic(), x, order, 0.01).map(|w| w.to_string());
        assert_eq!(wyckoff([0.0, 0.0, 0.0], 48).as_deref(), Some("1a"));
        assert_eq!(wyckoff([0.5, 0.5, 0.5], 48).as_deref(), Some("1b"));
        assert_eq!(wyckoff([0.0, 0.5, 0.5], 16).as_deref(), Some("3c"));
        assert_eq!(wyckoff([0.5, 0.0, 0.0], 16).as_deref(), Some("3d"));
        assert_eq!(wyckoff([0.2, 0.5, 0.5], 8).as_deref(), Some("6f"));
        assert_eq!(wyckoff([0.2, 0.0, 0.0], 8).as_deref(), Some("6e"));
    }

    /// Cell parameters the operations preserve, from a generic cell.
    fn preserved_cell(symmetry: &Symmetry) -> CellParams {
        let generic = CellParams::new(5.1, 6.2, 7.3, 81.0, 86.0, 97.0);
        let g = constraints::symmetrized_metric(symmetry, generic.metric());
        let length = |i: usize| g[i][i].sqrt();
        let angle = |i: usize, j: usize| (g[i][j] / (length(i) * length(j))).acos().to_degrees();
        CellParams::new(
            length(0),
            length(1),
            length(2),
            angle(1, 2),
            angle(0, 2),
            angle(0, 1),
        )
    }

    /// Every tabulated setting is identified as itself, and a generic point
    /// of every tabulated Wyckoff position is assigned that position, with
    /// the multiplicity the expansion gives.
    #[test]
    fn every_tabulated_wyckoff_position_is_recognized() {
        // No near relations such as y ≈ 2x or x + y ≈ 1/2, which would
        // put the point within the tolerance of a higher-symmetry one.
        let generic = [0.0713, 0.3291, 0.1847];
        let mut positions = 0;
        for hall_number in 1..=530 {
            let setting = tables::setting(hall_number);
            let symmetry = &setting.symmetry;
            let params = preserved_cell(symmetry);
            let sg = SpaceGroupType::identify(symmetry, &params).unwrap();
            assert_eq!(sg.number, setting.number, "Hall {hall_number}");
            // Three pairs of settings of No. 68 share their operations.
            assert!(
                sg.hall_number == hall_number || setting.number == 68,
                "Hall {hall_number}: {}",
                sg.hall_number
            );
            let metric = params.metric();
            for entry in tables::wyckoff_positions(hall_number) {
                let x = entry.coordinates.apply(generic);
                let stabilizer = crate::site::Stabilizer::new(symmetry, &metric, x, 0.01);
                let label = format!("Hall {hall_number} {}{}", entry.multiplicity, entry.letter);
                assert_eq!(
                    symmetry.operations().len() / stabilizer.order(),
                    entry.multiplicity,
                    "{label}"
                );
                // The duplicates of No. 68 are labeled by the other table.
                if sg.hall_number == hall_number {
                    let wyckoff = sg.wyckoff(&params, x, stabilizer.order(), 0.01);
                    assert_eq!(wyckoff.map(|w| w.letter), Some(entry.letter), "{label}");
                }
                positions += 1;
            }
        }
        assert_eq!(positions, 3467);
    }

    #[test]
    fn operations_from_names() {
        let count = |s: Option<Symmetry>| s.map(|s| s.operations().len());
        assert_eq!(count(Symmetry::from_hm_symbol("F m -3 m")), Some(192));
        assert_eq!(count(Symmetry::from_hall_symbol("-P 4n 2n")), Some(16));
        assert_eq!(count(Symmetry::from_it_number(194)), Some(24));
        assert_eq!(count(Symmetry::from_it_number(231)), None);
        assert_eq!(it_number_of("P 63/m m c"), Some(194));
    }
}
