use serde::{Deserialize, Serialize};

use crate::{Lattice, Measured, Symmetry};

/// Stable identity of a site within one structure; never reused and
/// independent of the site's label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SiteId(pub u32);

/// A chemical species: element symbol plus optional oxidation state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Species {
    /// An element symbol, or [`crate::UNKNOWN_ELEMENT`] when the source names
    /// none (design §4.5).
    pub element: String,
    pub oxidation: Option<i8>,
    /// The symbol as the source wrote it, e.g. `Fe3+` or `Q1`.
    pub symbol: String,
}

impl Species {
    pub fn element(symbol: &str) -> Self {
        Self::new(symbol, None, symbol)
    }

    pub fn new(element: &str, oxidation: Option<i8>, symbol: &str) -> Self {
        Self {
            element: element.to_string(),
            oxidation,
            symbol: symbol.to_string(),
        }
    }
}

/// Atomic displacement parameters in Å² (design §4.5). Kept as imported; not
/// edited in v0.x.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adp {
    Isotropic(Measured),
    /// U11, U22, U33, U12, U13, U23.
    Anisotropic([Measured; 6]),
}

/// One species on a site. Mixed occupancy is several occupants on one site.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Occupant {
    /// Unique within the structure, as CIF `_atom_site_label`.
    pub label: String,
    pub species: Species,
    pub occupancy: Measured,
    pub adp: Option<Adp>,
}

impl Occupant {
    pub fn new(label: &str, species: Species, occupancy: Measured) -> Self {
        Self {
            label: label.to_string(),
            species,
            occupancy,
            adp: None,
        }
    }

    pub fn with_adp(mut self, adp: Adp) -> Self {
        self.adp = Some(adp);
        self
    }
}

/// Membership of a split site in a disorder model, as CIF
/// `_atom_site_disorder_assembly` and `_atom_site_disorder_group`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Disorder {
    pub assembly: Option<String>,
    pub group: String,
}

impl Disorder {
    pub fn new(assembly: Option<String>, group: String) -> Self {
        Self { assembly, group }
    }
}

/// A position of the asymmetric unit, in fractional coordinates. The
/// coordinates are stored as given, not wrapped into the cell (design §3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Site {
    pub id: SiteId,
    pub position: [f64; 3],
    pub occupants: Vec<Occupant>,
    pub disorder: Option<Disorder>,
}

impl Site {
    pub fn new(id: SiteId, position: [f64; 3], occupants: Vec<Occupant>) -> Self {
        Self {
            id,
            position,
            occupants,
            disorder: None,
        }
    }

    pub fn with_disorder(mut self, disorder: Disorder) -> Self {
        self.disorder = Some(disorder);
        self
    }

    pub(crate) fn single(id: u32, label: &str, element: &str, position: [f64; 3]) -> Self {
        Self::new(
            SiteId(id),
            position,
            vec![Occupant::new(
                label,
                Species::element(element),
                Measured::exact(1.0),
            )],
        )
    }

    /// The first occupant's label, used to name the site.
    pub fn label(&self) -> &str {
        self.occupants.first().map_or("", |o| o.label.as_str())
    }

    /// The first occupant's element, used to draw the site.
    pub fn element(&self) -> &str {
        self.occupants
            .first()
            .map_or(crate::UNKNOWN_ELEMENT, |o| o.species.element.as_str())
    }
}

/// The space group as its source names it, kept for provenance and export.
/// The operations take precedence (design §4.4): the type identified from
/// them is [`crate::SymmetryAnalysis::space_group`], and the importer
/// reports a name that disagrees.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct SpaceGroup {
    /// Hermann–Mauguin symbol as written, e.g. `P 42/m n m`.
    pub hm_symbol: Option<String>,
    pub it_number: Option<u16>,
}

impl SpaceGroup {
    pub fn new(hm_symbol: Option<String>, it_number: Option<u16>) -> Self {
        Self {
            hm_symbol,
            it_number,
        }
    }
}

/// Distances that decide when two positions count as the same (design §4.6).
/// Stored with the structure so that every machine derives the same result.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Tolerances {
    /// Å. Equivalent positions closer than this are one atom; a site this
    /// close to a special position belongs on it.
    pub position: f64,
    /// Å. How far an atom may be from its symmetric image when searching
    /// the symmetry of atoms (design §7.10).
    pub symprec: f64,
    /// Å. Atoms closer than this are reported as too close.
    pub short_contact: f64,
}

impl Default for Tolerances {
    fn default() -> Self {
        Self {
            position: 0.01,
            symprec: 0.01,
            short_contact: 0.5,
        }
    }
}

/// Whether edits apply to whole orbits or to single atoms (design §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMode {
    Symmetric,
    P1,
}

/// A crystal structure: the asymmetric unit plus the operations that
/// generate the rest of the crystal (design §4.1).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Structure {
    pub lattice: Lattice,
    pub symmetry: Symmetry,
    pub space_group: SpaceGroup,
    pub sites: Vec<Site>,
    pub tolerances: Tolerances,
}

impl Structure {
    pub fn new(lattice: Lattice, symmetry: Symmetry, sites: Vec<Site>) -> Self {
        Self {
            lattice,
            symmetry,
            space_group: SpaceGroup::default(),
            sites,
            tolerances: Tolerances::default(),
        }
    }

    pub fn with_space_group(mut self, space_group: SpaceGroup) -> Self {
        self.space_group = space_group;
        self
    }

    /// P1 is the operation set that holds only the identity (design §4.4).
    pub fn edit_mode(&self) -> EditMode {
        if self.symmetry.is_p1() {
            EditMode::P1
        } else {
            EditMode::Symmetric
        }
    }

    pub fn site(&self, id: SiteId) -> Option<&Site> {
        self.sites.iter().find(|site| site.id == id)
    }
}
