use crate::expand::{self, Orbit};
use crate::{SiteId, Structure, element};

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

/// How a site's special position constrains one fractional coordinate
/// (design §7.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CoordinateConstraint {
    Free,
    /// Determined by another coordinate, e.g. y = x.
    Follows(Axis),
    Fixed,
}

/// The symmetry of one site: its Wyckoff position and how it constrains
/// the coordinates (design §5.2).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SiteSymmetry {
    pub wyckoff: String,
    /// The Wyckoff coordinate expression, e.g. `x, x, 0`.
    pub coordinate_form: String,
    pub coordinates: [CoordinateConstraint; 3],
}

/// One atom of the displayed structure, in Cartesian Å.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct DisplayAtom {
    pub site: SiteId,
    pub element: String,
    pub position: [f64; 3],
}

/// What the symmetry engine derives from the operation set (design §5.2,
/// §7.3): crystal system, cell constraints, Wyckoff positions.
///
/// Mock: the engine comes with M2. Until then only the built-in samples
/// have this, written by hand; imported structures have none.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SymmetryAnalysis {
    pub crystal_system: CrystalSystem,
    pub cell: CellConstraints,
    pub sites: Vec<(SiteId, SiteSymmetry)>,
}

/// Data derived from the structure (design §5).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Derived {
    /// `None` until the symmetry engine exists (M2), except for samples.
    pub symmetry: Option<SymmetryAnalysis>,
    pub orbits: Vec<Orbit>,
    /// Atoms in the displayed range, including boundary images.
    pub atoms: Vec<DisplayAtom>,
    /// g/cm³; `None` when a species has no known atomic weight.
    pub density: Option<f64>,
}

const AVOGADRO: f64 = 6.022_140_76e23;

impl Derived {
    /// Expands the structure. The symmetry analysis is left empty.
    pub fn compute(structure: &Structure) -> Self {
        let orbits = expand::orbits(structure);
        let atoms = expand::display_atoms(structure, &orbits);
        let density = density(structure, &orbits);
        Self {
            symmetry: None,
            orbits,
            atoms,
            density,
        }
    }

    pub fn with_symmetry(mut self, symmetry: SymmetryAnalysis) -> Self {
        self.symmetry = Some(symmetry);
        self
    }

    pub fn site_symmetry(&self, id: SiteId) -> Option<&SiteSymmetry> {
        self.symmetry
            .as_ref()?
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
