use crate::{Lattice, Measured};

/// Stable identity of a site within one structure; never reused and
/// independent of the site's label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SiteId(pub u32);

/// A chemical species: element symbol plus optional oxidation state.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Species {
    pub element: String,
    pub oxidation: Option<i8>,
}

impl Species {
    pub fn element(symbol: &str) -> Self {
        Self {
            element: symbol.to_string(),
            oxidation: None,
        }
    }
}

/// One species on a site. Mixed occupancy is several occupants on one site.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Occupant {
    /// Unique within the structure, as CIF `_atom_site_label`.
    pub label: String,
    pub species: Species,
    pub occupancy: Measured,
}

/// A position of the asymmetric unit, in fractional coordinates.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Site {
    pub id: SiteId,
    pub position: [f64; 3],
    pub occupants: Vec<Occupant>,
}

impl Site {
    pub(crate) fn single(id: u32, label: &str, element: &str, position: [f64; 3]) -> Self {
        Self {
            id: SiteId(id),
            position,
            occupants: vec![Occupant {
                label: label.to_string(),
                species: Species::element(element),
                occupancy: Measured::exact(1.0),
            }],
        }
    }

    /// The first occupant's label, used to name the site.
    pub fn label(&self) -> &str {
        self.occupants.first().map_or("", |o| o.label.as_str())
    }
}

/// Space group identity. The symmetry operations, which are the actual source
/// of truth in the design, come with the symmetry engine.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SpaceGroup {
    pub hm_symbol: String,
    pub it_number: u16,
}

/// Whether edits apply to whole orbits or to single atoms (design §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMode {
    Symmetric,
    P1,
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Structure {
    pub lattice: Lattice,
    pub space_group: SpaceGroup,
    pub sites: Vec<Site>,
}

impl Structure {
    pub fn edit_mode(&self) -> EditMode {
        if self.space_group.it_number == 1 {
            EditMode::P1
        } else {
            EditMode::Symmetric
        }
    }

    pub fn site(&self, id: SiteId) -> Option<&Site> {
        self.sites.iter().find(|site| site.id == id)
    }
}
