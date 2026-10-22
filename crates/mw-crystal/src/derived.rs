use crate::expand::{self, Orbit};
use crate::site::Stabilizer;
use crate::space_group::{self, DetectedSymmetry, SpaceGroupType};
use crate::{
    AtomRef, Bond, BondRules, SiteId, SiteSymmetry, Structure, bonds, constraints, element,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrystalSystem {
    Triclinic,
    Monoclinic,
    Orthorhombic,
    Tetragonal,
    Trigonal,
    Hexagonal,
    Cubic,
}

impl CrystalSystem {
    /// The crystal system of an International Tables number.
    pub(crate) fn of(number: u16) -> Self {
        match number {
            0..=2 => Self::Triclinic,
            3..=15 => Self::Monoclinic,
            16..=74 => Self::Orthorhombic,
            75..=142 => Self::Tetragonal,
            143..=167 => Self::Trigonal,
            168..=194 => Self::Hexagonal,
            _ => Self::Cubic,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellParam {
    A,
    B,
    C,
    Alpha,
    Beta,
    Gamma,
}

impl CellParam {
    pub const ALL: [CellParam; 6] = [
        CellParam::A,
        CellParam::B,
        CellParam::C,
        CellParam::Alpha,
        CellParam::Beta,
        CellParam::Gamma,
    ];
}

/// How symmetry constrains one cell parameter (design §7.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Constraint {
    Free,
    /// Always equal to another parameter, e.g. b = a.
    Equals(CellParam),
    /// Fixed by the crystal system, e.g. 90°.
    Fixed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellConstraints([Constraint; 6]);

impl CellConstraints {
    pub fn new(constraints: [Constraint; 6]) -> Self {
        Self(constraints)
    }

    pub fn free() -> Self {
        Self([Constraint::Free; 6])
    }

    pub fn get(&self, param: CellParam) -> Constraint {
        self.0[param as usize]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];
}

/// How a site's special position constrains one fractional coordinate
/// (design §7.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoordinateConstraint {
    Free,
    /// Determined by one free coordinate, e.g. y = x or y = 2x.
    Follows(Axis),
    /// Determined by two free coordinates, which only non-standard
    /// settings produce.
    Combination,
    Fixed,
}

/// One atom of the displayed structure, in Cartesian Å.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct DisplayAtom {
    pub atom: AtomRef,
    pub element: String,
    pub position: [f64; 3],
}

/// What the operation set implies (design §5.2, §7.3): the space group
/// type, the cell constraints and every site's symmetry.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SymmetryAnalysis {
    /// `None` only if identification fails, which a valid operation set
    /// should never cause.
    pub space_group: Option<SpaceGroupType>,
    pub cell: CellConstraints,
    pub sites: Vec<(SiteId, SiteSymmetry)>,
    /// For a structure in P1, the symmetry its atoms have (design §7.10).
    /// `None` for other structures, and for P1 structures with more than
    /// [`DETECTION_LIMIT`] atoms.
    pub detected: Option<DetectedSymmetry>,
}

/// Atoms beyond which a P1 structure's symmetry is not searched when it is
/// opened; the search grows faster than linearly.
pub const DETECTION_LIMIT: usize = 2000;

impl SymmetryAnalysis {
    pub fn compute(structure: &Structure, orbits: &[Orbit]) -> Self {
        let params = structure.lattice.params();
        let metric = params.metric();
        let tolerance = structure.tolerances.position;
        let space_group = SpaceGroupType::identify(&structure.symmetry, params);
        let sites = structure
            .sites
            .iter()
            .map(|site| {
                let stabilizer =
                    Stabilizer::new(&structure.symmetry, &metric, site.position, tolerance);
                let wyckoff = space_group.as_ref().and_then(|sg| {
                    sg.wyckoff(params, site.position, stabilizer.order(), tolerance)
                });
                (site.id, SiteSymmetry::new(&stabilizer, wyckoff))
            })
            .collect();
        let atoms: usize = orbits.iter().map(|o| o.positions.len()).sum();
        let detected = (structure.symmetry.is_p1() && atoms <= DETECTION_LIMIT)
            .then(|| space_group::detect_symmetry(structure, orbits))
            .flatten();
        Self {
            space_group,
            cell: constraints::cell_constraints(&structure.symmetry),
            sites,
            detected,
        }
    }

    pub fn crystal_system(&self) -> Option<CrystalSystem> {
        self.space_group
            .as_ref()
            .map(SpaceGroupType::crystal_system)
    }
}

/// Data derived from the structure (design §5).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Derived {
    pub symmetry: SymmetryAnalysis,
    pub orbits: Vec<Orbit>,
    /// Atoms in the displayed range, including boundary images.
    pub atoms: Vec<DisplayAtom>,
    /// Bonds of the crystal, once per unit cell, under the default rules.
    pub bonds: Vec<Bond>,
    /// Bonds between displayed atoms, as index pairs into `atoms`.
    pub display_bonds: Vec<[usize; 2]>,
    /// g/cm³; `None` when a species has no known atomic weight.
    pub density: Option<f64>,
}

const AVOGADRO: f64 = 6.022_140_76e23;

impl Derived {
    /// Expands the structure, finds its bonds and analyzes its symmetry.
    pub fn compute(structure: &Structure) -> Self {
        let orbits = expand::orbits(structure);
        let atoms = expand::display_atoms(structure, &orbits);
        let bonds = bonds::bonds(structure, &orbits, &BondRules::default());
        let display_bonds = bonds::display_bonds(&atoms, &bonds);
        let density = density(structure, &orbits);
        Self {
            symmetry: SymmetryAnalysis::compute(structure, &orbits),
            orbits,
            atoms,
            bonds,
            display_bonds,
            density,
        }
    }

    pub fn site_symmetry(&self, id: SiteId) -> Option<&SiteSymmetry> {
        self.symmetry
            .sites
            .iter()
            .find_map(|(site, symmetry)| (*site == id).then_some(symmetry))
    }

    pub fn orbit(&self, id: SiteId) -> Option<&Orbit> {
        self.orbits.iter().find(|orbit| orbit.site == id)
    }

    /// Number of equivalent atoms of a site in the unit cell.
    pub fn multiplicity(&self, id: SiteId) -> Option<usize> {
        self.orbit(id).map(|orbit| orbit.positions.len())
    }

    /// Number of atoms in the unit cell, without boundary images.
    pub fn atoms_in_cell(&self) -> usize {
        self.orbits.iter().map(|orbit| orbit.positions.len()).sum()
    }
}

/// Mass of the cell contents (orbit size × occupancy × atomic weight) over
/// its volume.
fn density(structure: &Structure, orbits: &[Orbit]) -> Option<f64> {
    let mut mass = 0.0;
    for orbit in orbits {
        let site = structure.site(orbit.site)?;
        for occupant in &site.occupants {
            let weight = element::atomic_weight(&occupant.species.element)?;
            mass += orbit.positions.len() as f64 * occupant.occupancy.value() * weight;
        }
    }
    let volume = structure.lattice.params().volume();
    (volume > 0.0).then(|| mass / (AVOGADRO * volume * 1e-24))
}
