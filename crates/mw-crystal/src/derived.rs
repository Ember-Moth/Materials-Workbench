use crate::SiteId;

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

#[derive(Clone, Debug, PartialEq)]
pub struct SiteSymmetry {
    pub wyckoff: String,
    pub multiplicity: u32,
    /// The Wyckoff coordinate expression, e.g. `x, x, 0`.
    pub coordinate_form: String,
    pub coordinates: [CoordinateConstraint; 3],
}

/// One atom of the displayed structure, in Cartesian Å.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayAtom {
    pub site: SiteId,
    pub element: String,
    pub position: [f64; 3],
}

/// Data the design derives from the structure (design §5).
///
/// Mock: until the symmetry engine exists this is filled in by hand for the
/// samples, so it is only as correct as the sample that supplies it.
#[derive(Clone, Debug, PartialEq)]
pub struct Derived {
    pub crystal_system: CrystalSystem,
    pub cell: CellConstraints,
    pub sites: Vec<(SiteId, SiteSymmetry)>,
    /// Atoms in the displayed range, including boundary images.
    pub atoms: Vec<DisplayAtom>,
    pub formula: String,
    /// g/cm³.
    pub density: f64,
}

impl Derived {
    pub fn site(&self, id: SiteId) -> Option<&SiteSymmetry> {
        self.sites
            .iter()
            .find_map(|(site, symmetry)| (*site == id).then_some(symmetry))
    }
}
