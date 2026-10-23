use std::collections::HashSet;
use std::fmt;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    /// Standard uncertainty of each coordinate, as CIF `0.3048(2)`; `None`
    /// for coordinates given without one. Kept for export only (design
    /// §4.3): an edited coordinate has none.
    #[serde(default, skip_serializing_if = "no_su")]
    pub position_su: [Option<f64>; 3],
    pub occupants: Vec<Occupant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disorder: Option<Disorder>,
}

fn no_su(su: &[Option<f64>; 3]) -> bool {
    su.iter().all(Option::is_none)
}

impl Site {
    pub fn new(id: SiteId, position: [f64; 3], occupants: Vec<Occupant>) -> Self {
        Self {
            id,
            position,
            position_su: [None; 3],
            occupants,
            disorder: None,
        }
    }

    pub fn with_disorder(mut self, disorder: Disorder) -> Self {
        self.disorder = Some(disorder);
        self
    }

    pub fn with_position_su(mut self, su: [Option<f64>; 3]) -> Self {
        self.position_su = su;
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SpaceGroup {
    /// Hermann–Mauguin symbol as written, e.g. `P 42/m n m`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hm_symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub it_number: Option<u16>,
}

impl SpaceGroup {
    pub fn new(hm_symbol: Option<String>, it_number: Option<u16>) -> Self {
        Self {
            hm_symbol,
            it_number,
        }
    }

    fn is_empty(&self) -> bool {
        self.hm_symbol.is_none() && self.it_number.is_none()
    }
}

/// Distances that decide when two positions count as the same (design §4.6).
/// Stored with the structure so that every machine derives the same result.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
///
/// Serialized as project files store it (`docs/design/project-format.md`
/// §6): the cell parameters, the operations in CIF notation and in their
/// order (atom references depend on it), and the sites. Reading checks that
/// the operations form a group; [`Structure::validate`] checks the rest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Structure {
    #[serde(rename = "cell")]
    pub lattice: Lattice,
    #[serde(rename = "operations")]
    pub symmetry: Symmetry,
    #[serde(default, skip_serializing_if = "SpaceGroup::is_empty")]
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

    /// Checks the hard constraints of the model (design §8) that the types
    /// alone do not guarantee: for a structure read from a file rather than
    /// built by the importer or edit commands. The operation set is checked
    /// when it is read.
    pub fn validate(&self) -> Result<(), StructureError> {
        let params = self.lattice.params();
        let cell = [
            params.a,
            params.b,
            params.c,
            params.alpha,
            params.beta,
            params.gamma,
        ];
        if !cell.iter().all(|m| finite(*m)) || !params.is_valid() {
            return Err(StructureError::InvalidCell);
        }
        let tolerances = &self.tolerances;
        if ![
            tolerances.position,
            tolerances.symprec,
            tolerances.short_contact,
        ]
        .iter()
        .all(|t| t.is_finite() && *t > 0.0)
        {
            return Err(StructureError::InvalidTolerances);
        }
        let mut ids = HashSet::new();
        let mut labels = HashSet::new();
        for site in &self.sites {
            if !ids.insert(site.id) {
                return Err(StructureError::DuplicateSiteId(site.id));
            }
            if site.occupants.is_empty() {
                return Err(StructureError::EmptySite(site.id));
            }
            let su_valid = site
                .position_su
                .iter()
                .flatten()
                .all(|su| su.is_finite() && *su >= 0.0);
            if !site.position.iter().all(|x| x.is_finite()) || !su_valid {
                return Err(StructureError::InvalidPosition(site.id));
            }
            for occupant in &site.occupants {
                if !labels.insert(occupant.label.as_str()) {
                    return Err(StructureError::DuplicateLabel(occupant.label.clone()));
                }
                // Above 1 is a warning on import, not an error (design §7.7).
                let occupancy = occupant.occupancy;
                if !finite(occupancy) || occupancy.value() <= 0.0 {
                    return Err(StructureError::InvalidOccupancy {
                        label: occupant.label.clone(),
                        value: occupancy.value(),
                    });
                }
            }
        }
        Ok(())
    }
}

fn finite(value: Measured) -> bool {
    value.value().is_finite() && value.su().is_none_or(f64::is_finite)
}

/// Why a structure breaks the model's hard constraints.
#[derive(Clone, Debug, PartialEq)]
pub enum StructureError {
    /// A cell parameter is not a finite number, or the cell cannot exist.
    InvalidCell,
    InvalidTolerances,
    DuplicateSiteId(SiteId),
    /// A site without occupants.
    EmptySite(SiteId),
    /// A coordinate or its uncertainty is not a finite number.
    InvalidPosition(SiteId),
    DuplicateLabel(String),
    InvalidOccupancy {
        label: String,
        value: f64,
    },
}

impl fmt::Display for StructureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCell => write!(f, "the cell parameters describe no cell"),
            Self::InvalidTolerances => write!(f, "tolerances must be positive"),
            Self::DuplicateSiteId(id) => write!(f, "site id {} is used twice", id.0),
            Self::EmptySite(id) => write!(f, "site {} has no occupant", id.0),
            Self::InvalidPosition(id) => {
                write!(f, "site {} has a coordinate that is not a number", id.0)
            }
            Self::DuplicateLabel(label) => write!(f, "label {label} is used twice"),
            Self::InvalidOccupancy { label, value } => {
                write!(f, "occupancy {value} of {label} is not positive")
            }
        }
    }
}

impl std::error::Error for StructureError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CellParams;
    use crate::sample::{self, SampleKey};

    fn round_trip(structure: &Structure) -> Structure {
        let json = serde_json::to_string(structure).unwrap();
        serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}: {json}"))
    }

    #[test]
    fn samples_serialize_exactly() {
        for key in [
            SampleKey::Rutile,
            SampleKey::RutileSupercell,
            SampleKey::StrontiumTitanate,
        ] {
            let structure = sample::load(key).structure;
            assert_eq!(round_trip(&structure), structure, "{key:?}");
            assert_eq!(structure.validate(), Ok(()));
        }
    }

    #[test]
    fn serialized_form() {
        let mut structure = sample::load(SampleKey::StrontiumTitanate).structure;
        structure.sites.truncate(1);
        structure.sites[0].position_su = [Some(0.0002), None, None];
        structure.sites[0].occupants[0].occupancy = Measured::with_su(0.98, 0.01);
        let json = serde_json::to_value(&structure).unwrap();
        assert_eq!(json["cell"]["a"], serde_json::json!({"value": 3.905}));
        assert_eq!(json["operations"].as_array().unwrap().len(), 48);
        assert_eq!(json["operations"][0], "x,y,z");
        assert_eq!(
            json["space_group"],
            serde_json::json!({"hm_symbol": "P m -3 m", "it_number": 221})
        );
        assert_eq!(
            json["sites"][0],
            serde_json::json!({
                "id": 1,
                "position": [0.0, 0.0, 0.0],
                "position_su": [0.0002, null, null],
                "occupants": [{
                    "label": "Sr1",
                    "species": {"element": "Sr", "symbol": "Sr"},
                    "occupancy": {"value": 0.98, "su": 0.01}
                }]
            })
        );
        assert_eq!(
            json["tolerances"],
            serde_json::json!({"position": 0.01, "symprec": 0.01, "short_contact": 0.5})
        );
        assert_eq!(round_trip(&structure), structure);
    }

    #[test]
    fn operations_that_are_no_group_are_refused() {
        let structure = sample::load(SampleKey::Rutile).structure;
        let mut json = serde_json::to_value(&structure).unwrap();
        json["operations"].as_array_mut().unwrap().pop();
        let error = serde_json::from_value::<Structure>(json).unwrap_err();
        assert!(error.to_string().contains("is missing"), "{error}");
        let mut json = serde_json::to_value(&structure).unwrap();
        json["operations"][1] = "x,y,q".into();
        assert!(serde_json::from_value::<Structure>(json).is_err());
    }

    #[test]
    fn validation_finds_broken_constraints() {
        let structure = sample::load(SampleKey::StrontiumTitanate).structure;
        let broken = |change: fn(&mut Structure)| {
            let mut structure = structure.clone();
            change(&mut structure);
            structure.validate().unwrap_err()
        };
        assert_eq!(
            broken(|s| s
                .lattice
                .set_params(CellParams::new(1.0, 1.0, 1.0, 10.0, 10.0, 170.0))),
            StructureError::InvalidCell
        );
        assert_eq!(
            broken(|s| s.sites[1].id = SiteId(1)),
            StructureError::DuplicateSiteId(SiteId(1))
        );
        assert_eq!(
            broken(|s| s.sites[2].occupants.clear()),
            StructureError::EmptySite(SiteId(3))
        );
        assert_eq!(
            broken(|s| s.sites[0].position[1] = f64::NAN),
            StructureError::InvalidPosition(SiteId(1))
        );
        assert_eq!(
            broken(|s| s.sites[1].occupants[0].label = "Sr1".into()),
            StructureError::DuplicateLabel("Sr1".into())
        );
        assert!(matches!(
            broken(|s| s.sites[0].occupants[0].occupancy = Measured::exact(0.0)),
            StructureError::InvalidOccupancy { .. }
        ));
        assert_eq!(
            broken(|s| s.tolerances.position = 0.0),
            StructureError::InvalidTolerances
        );
    }
}
