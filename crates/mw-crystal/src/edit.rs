//! Editing commands (design §7.2). An [`Edit`] is serializable data that
//! names whole sites. Applying it gives the new structure, the edit that
//! restores the old one exactly, what became of the sites (design §7.11)
//! and warnings. [`target_sites`] decides which sites a request that names
//! single atoms acts on, and refuses requests that would break the symmetry
//! (design §7.1).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::bonds::{alternatives, covalent_radius};
use crate::constraints::{cell_constraints, symmetrized_metric};
use crate::placement::{self, Placement};
use crate::{
    AtomFate, AtomRef, Axis, CellParam, CellParams, CellTransform, Constraint,
    CoordinateConstraint, Derived, DetectSymmetry, EditMode, IdMap, Measured, Occupant,
    PositionMap, Site, SiteFate, SiteId, Species, Structure, SymmetrizeError, TransformError,
    element, expand, special_position, transform,
};

/// How far the occupancies of one site may add up above 1 before an edit
/// warns; refinements often end slightly above 1 (design §7.7).
pub const OCCUPANCY_TOLERANCE: f64 = 0.01;

/// A cell parameter given again within this many Å or degrees of its
/// value counts as unchanged, and so does a fractional coordinate.
const SAME_VALUE: f64 = 1e-9;

/// Å. Snapping a site by less than this is not reported: it is below the
/// precision of coordinates given to 4–5 decimals, as the importer judges
/// too.
const REPORTED_SNAP: f64 = 1e-4;

/// New values for some cell parameters, in Å and degrees. A parameter left
/// out keeps its value, or follows the parameter it is linked to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CellValues {
    pub a: Option<f64>,
    pub b: Option<f64>,
    pub c: Option<f64>,
    pub alpha: Option<f64>,
    pub beta: Option<f64>,
    pub gamma: Option<f64>,
}

impl CellValues {
    pub fn get(&self, param: CellParam) -> Option<f64> {
        match param {
            CellParam::A => self.a,
            CellParam::B => self.b,
            CellParam::C => self.c,
            CellParam::Alpha => self.alpha,
            CellParam::Beta => self.beta,
            CellParam::Gamma => self.gamma,
        }
    }

    pub fn with(mut self, param: CellParam, value: f64) -> Self {
        let field = match param {
            CellParam::A => &mut self.a,
            CellParam::B => &mut self.b,
            CellParam::C => &mut self.c,
            CellParam::Alpha => &mut self.alpha,
            CellParam::Beta => &mut self.beta,
            CellParam::Gamma => &mut self.gamma,
        };
        *field = Some(value);
        self
    }
}

/// One editing command (design §7.2). Occupants are named by their labels,
/// which are unique within a structure. The `Restore*` commands are the
/// exact inverses of others; they put back what an edit took away,
/// uncertainties and displacement parameters included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Edit {
    /// Changes cell parameters; fractional coordinates stay, so the atoms
    /// scale with the cell (design §7.3). A parameter that symmetry links
    /// to another changes with it; a fixed one cannot change. Typed values
    /// carry no uncertainty.
    SetCell { values: CellValues },
    /// Sets all six parameters as given, without constraints.
    RestoreCell { params: CellParams },
    /// Replaces an occupant's species on all equivalent atoms. The label
    /// stays (design §7.7).
    SetSpecies {
        site: SiteId,
        occupant: String,
        species: Species,
    },
    /// Sets an occupant's occupancy, which must be in (0, 1].
    SetOccupancy {
        site: SiteId,
        occupant: String,
        occupancy: Measured,
    },
    /// Adds an occupant to a site, making its occupancy mixed (design
    /// §7.7). Without a label, the element symbol and the smallest unused
    /// number make one (design §7.6).
    AddOccupant {
        site: SiteId,
        species: Species,
        occupancy: Measured,
        label: Option<String>,
    },
    /// Removes an occupant; a site keeps at least one.
    RemoveOccupant { site: SiteId, occupant: String },
    RestoreOccupant {
        site: SiteId,
        index: usize,
        occupant: Occupant,
    },
    RenameLabel {
        site: SiteId,
        occupant: String,
        label: String,
    },
    /// Moves a site to new fractional coordinates. Under symmetry the site
    /// takes the site symmetry of its new position: within the position
    /// tolerance of a special position, it snaps onto it (design §7.4).
    SetPosition { site: SiteId, position: [f64; 3] },
    /// Drags one atom by a fractional displacement, and all the atoms
    /// equivalent to it with it (design §7.4). The displacement is taken
    /// back to the site and projected onto the directions its special
    /// position allows, so the site stays on it; with
    /// `leave_special_position` it is not projected, and the site symmetry
    /// may drop. A site that ends up near a special position with more
    /// symmetry snaps onto it.
    MoveSite {
        atom: AtomRef,
        displacement: [f64; 3],
        #[serde(default)]
        leave_special_position: bool,
    },
    /// Sets one fractional coordinate of a site without leaving its special
    /// position (design §7.5): a free coordinate, or one that follows a
    /// single free coordinate. The coordinates that depend on it follow; a
    /// fixed coordinate cannot change.
    SetCoordinate {
        site: SiteId,
        axis: Axis,
        value: f64,
    },
    /// Puts a site back where it was, uncertainties included, without
    /// projecting or snapping.
    RestorePosition {
        site: SiteId,
        position: [f64; 3],
        position_su: [Option<f64>; 3],
    },
    /// Adds a site to the end of the asymmetric unit, with the next unused
    /// id, snapped onto the special position it lies near (design §7.6). A
    /// site that would coincide with an atom of another is refused. Without
    /// a label, the element symbol and the smallest unused number make one.
    AddSite {
        position: [f64; 3],
        species: Species,
        occupancy: Measured,
        label: Option<String>,
    },
    /// Removes a site with all its equivalent atoms (design §7.6).
    RemoveSite { site: SiteId },
    /// Puts a site back at its place in the asymmetric unit.
    RestoreSite { index: usize, site: Site },
    /// Several edits as one, applied in order; all of them or none.
    Batch { edits: Vec<Edit> },
    /// Expands the structure to P1 (design §7.1): every equivalent atom
    /// becomes a site of its own, labeled `label_k` when its site had
    /// several. References to atoms follow them to their new sites.
    ExpandToP1,
    /// Puts back a whole structure: the inverse of [`Edit::ExpandToP1`].
    /// References to atoms follow them to the atoms at the same positions.
    RestoreStructure { structure: Box<Structure> },
    /// Describes the structure in another cell (design §7.8): the new basis
    /// is `A·P`, the new origin is at p in the old basis, and coordinates
    /// become `x' = P⁻¹·(x − p)`. The asymmetric unit stays the asymmetric
    /// unit, with the same sites and labels; a larger cell adds the old
    /// lattice translations as centering, a smaller one merges operations
    /// that differ by a new lattice translation. References to atoms follow
    /// them.
    TransformCell { transform: CellTransform },
    /// Puts back the structure a change of cell was applied to, exactly:
    /// the inverse of [`Edit::TransformCell`]. `transform` is the change of
    /// cell this undoes, inverted; references to atoms follow them through
    /// it.
    RestoreBasis {
        transform: CellTransform,
        structure: Box<Structure>,
    },
    /// Symmetrizes the structure by the symmetry its atoms have (design
    /// §7.10): the operations moyo finds within `symprec` Å (the
    /// structure's own tolerance when `None`), every atom averaged onto its
    /// symmetric position and the equivalent atoms merged into one site,
    /// occupancies and displacement parameters averaged too. The cell is
    /// kept, or with `standardize` the result is in the conventional cell
    /// of the standard setting. References to atoms follow them to the
    /// nearest atom.
    Symmetrize {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symprec: Option<f64>,
        #[serde(default)]
        standardize: bool,
    },
    /// Puts back a whole structure, the inverse of [`Edit::Symmetrize`]:
    /// `mapping` takes fractional coordinates of the current structure to
    /// those of the restored one, and references to atoms follow them to
    /// the nearest atom there.
    RestoreAtoms {
        mapping: PositionMap,
        structure: Box<Structure>,
    },
}

/// The result of applying an [`Edit`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Applied {
    pub structure: Structure,
    /// Applied to `structure`, gives back the structure the edit was applied
    /// to.
    pub inverse: Edit,
    pub ids: IdMap,
    pub warnings: Vec<EditWarning>,
    pub info: Vec<EditInfo>,
}

/// What an edit reports besides its result.
#[derive(Default)]
struct Report {
    warnings: Vec<EditWarning>,
    info: Vec<EditInfo>,
}

/// Something an edit did that the user should know about, without
/// refusing it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum EditWarning {
    /// The occupancies of a site add up to more than 1 (design §7.7).
    OccupancySum { site: SiteId, sum: f64 },
    /// The occupancies of a split site and of the split positions next to
    /// it in the other groups of its disorder assembly add up to more than
    /// 1 (design §7.7). `partners` holds, for each other group, the
    /// adjacent site with the largest occupancy.
    SplitOccupancySum {
        site: SiteId,
        partners: Vec<SiteId>,
        sum: f64,
    },
    /// After a move, an atom of `site` is closer than the structure's
    /// short-contact distance to an atom of `other`, in Å (design §7.4).
    ShortContact {
        site: SiteId,
        other: SiteId,
        distance: f64,
    },
    /// After a move, equivalent atoms of `site` are closer than the
    /// short-contact distance to each other, in Å: it left a special
    /// position by a little (design §7.4).
    CloseImages { site: SiteId, distance: f64 },
}

impl EditWarning {
    /// A stable identifier for programs and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::OccupancySum { .. } => "occupancy_sum",
            Self::SplitOccupancySum { .. } => "split_occupancy_sum",
            Self::ShortContact { .. } => "short_contact",
            Self::CloseImages { .. } => "close_images",
        }
    }
}

/// English, for programs, logs and AI callers.
impl fmt::Display for EditWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OccupancySum { site, sum } => write!(
                f,
                "the occupancies of site {} add up to {sum:.4}, more than 1",
                site.0
            ),
            Self::SplitOccupancySum {
                site,
                partners,
                sum,
            } => {
                let partners: Vec<String> = partners.iter().map(|p| p.0.to_string()).collect();
                write!(
                    f,
                    "the occupancies of split site {} and of the alternative positions next to \
                     it (sites {}) add up to {sum:.4}, more than 1",
                    site.0,
                    partners.join(", ")
                )
            }
            Self::ShortContact {
                site,
                other,
                distance,
            } => write!(
                f,
                "an atom of site {} is {distance:.3} Å from an atom of site {}",
                site.0, other.0
            ),
            Self::CloseImages { site, distance } => write!(
                f,
                "equivalent atoms of site {} are {distance:.3} Å apart",
                site.0
            ),
        }
    }
}

/// Something an edit did besides what it was asked, which the user should
/// see (design §8, Info): a position snapped, or a site whose number of
/// equivalent atoms changed.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum EditInfo {
    /// The site was pulled onto a special position nearby, by
    /// `displacement` Å (design §7.4, §7.6).
    Snapped { site: SiteId, displacement: f64 },
    /// The site left a special position or landed on one: it has `after`
    /// equivalent atoms in the cell instead of `before` (design §7.4).
    Multiplicity {
        site: SiteId,
        before: usize,
        after: usize,
    },
}

impl EditInfo {
    /// A stable identifier for programs and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Snapped { .. } => "snapped",
            Self::Multiplicity { .. } => "multiplicity",
        }
    }
}

/// English, for programs, logs and AI callers.
impl fmt::Display for EditInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Snapped { site, displacement } => write!(
                f,
                "site {} was moved {displacement:.4} Å onto the special position nearby",
                site.0
            ),
            Self::Multiplicity {
                site,
                before,
                after,
            } => write!(
                f,
                "site {} now has {after} equivalent atoms in the cell instead of {before}",
                site.0
            ),
        }
    }
}

/// A way forward from a refused edit (design §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Guidance {
    /// Edit every equivalent atom of the site.
    WholeSite { site: SiteId },
    /// Convert the structure to P1 (`ExpandToP1`), then edit single atoms.
    ConvertToP1,
    /// Remove the whole site.
    RemoveSite { site: SiteId },
    /// Add the species to this site as a mixed occupant instead.
    AddOccupant { site: SiteId },
}

/// Why an edit was refused. The structure is unchanged.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum EditError {
    UnknownSite(SiteId),
    UnknownAtom(AtomRef),
    UnknownOccupant {
        site: SiteId,
        label: String,
    },
    /// Under symmetry, an edit of `atoms` of a site's `equivalent` atoms
    /// would break the symmetry (design §7.1).
    BreaksSymmetry {
        site: SiteId,
        atoms: usize,
        equivalent: usize,
    },
    /// The site's special position fixes this coordinate, e.g. z = 1/4
    /// (design §7.5).
    FixedCoordinate {
        site: SiteId,
        axis: Axis,
    },
    /// The coordinate is a combination of two free coordinates, which only
    /// non-standard settings give; those can be set instead.
    CombinedCoordinate {
        site: SiteId,
        axis: Axis,
    },
    /// A new site would coincide with an atom of `site` (design §7.6).
    CoincidentSite {
        site: SiteId,
    },
    FixedCellParameter {
        param: CellParam,
    },
    /// Two parameters that symmetry links were given different values.
    LinkedCellParameters {
        param: CellParam,
        other: CellParam,
    },
    /// A length is not positive, an angle not strictly between 0° and
    /// 180°, or the metric tensor not positive definite.
    InvalidCell,
    InvalidPosition,
    OccupancyOutOfRange {
        value: f64,
    },
    LastOccupant {
        site: SiteId,
    },
    UnknownElement {
        symbol: String,
    },
    /// Empty, containing white space, or starting with a character CIF
    /// reserves.
    InvalidLabel {
        label: String,
    },
    DuplicateLabel {
        label: String,
    },
    DuplicateSite(SiteId),
    InvalidIndex {
        index: usize,
    },
    /// A measurement to remove that the document does not have.
    UnknownMeasurement,
    /// A measurement that names the same atom twice.
    RepeatedAtom(AtomRef),
    /// The structure is in P1 already.
    AlreadyP1,
    /// The change of cell does not fit the structure (design §7.8).
    Transform(TransformError),
    /// The structure cannot be symmetrized (design §7.10).
    Symmetrize(SymmetrizeError),
}

impl EditError {
    /// The ways forward, best first.
    pub fn guidance(&self) -> Vec<Guidance> {
        match *self {
            Self::BreaksSymmetry { site, .. } => {
                vec![Guidance::WholeSite { site }, Guidance::ConvertToP1]
            }
            Self::CoincidentSite { site } => vec![Guidance::AddOccupant { site }],
            Self::LastOccupant { site } => vec![Guidance::RemoveSite { site }],
            _ => Vec::new(),
        }
    }

    /// A stable identifier for programs and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownSite(_) => "unknown_site",
            Self::UnknownAtom(_) => "unknown_atom",
            Self::UnknownOccupant { .. } => "unknown_occupant",
            Self::BreaksSymmetry { .. } => "breaks_symmetry",
            Self::FixedCoordinate { .. } => "fixed_coordinate",
            Self::CombinedCoordinate { .. } => "combined_coordinate",
            Self::CoincidentSite { .. } => "coincident_site",
            Self::FixedCellParameter { .. } => "fixed_cell_parameter",
            Self::LinkedCellParameters { .. } => "linked_cell_parameters",
            Self::InvalidCell => "invalid_cell",
            Self::InvalidPosition => "invalid_position",
            Self::OccupancyOutOfRange { .. } => "occupancy_out_of_range",
            Self::LastOccupant { .. } => "last_occupant",
            Self::UnknownElement { .. } => "unknown_element",
            Self::InvalidLabel { .. } => "invalid_label",
            Self::DuplicateLabel { .. } => "duplicate_label",
            Self::DuplicateSite(_) => "duplicate_site",
            Self::InvalidIndex { .. } => "invalid_index",
            Self::UnknownMeasurement => "unknown_measurement",
            Self::RepeatedAtom(_) => "repeated_atom",
            Self::AlreadyP1 => "already_p1",
            Self::Transform(error) => error.code(),
            Self::Symmetrize(error) => error.code(),
        }
    }
}

fn param_name(param: CellParam) -> &'static str {
    match param {
        CellParam::A => "a",
        CellParam::B => "b",
        CellParam::C => "c",
        CellParam::Alpha => "alpha",
        CellParam::Beta => "beta",
        CellParam::Gamma => "gamma",
    }
}

/// English, for programs, logs and AI callers; the interface translates
/// the variants itself.
impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSite(site) => write!(f, "no site with id {}", site.0),
            Self::UnknownAtom(atom) => write!(
                f,
                "site {} has no atom {} in cell {:?}",
                atom.site.0, atom.member, atom.cell
            ),
            Self::UnknownOccupant { site, label } => {
                write!(f, "site {} has no occupant labelled {label:?}", site.0)
            }
            Self::BreaksSymmetry {
                site,
                atoms,
                equivalent,
            } => write!(
                f,
                "editing {atoms} of the {equivalent} equivalent atoms of site {} would break \
                 the symmetry; edit the whole site, or convert the structure to P1 first",
                site.0
            ),
            Self::FixedCoordinate { site, axis } => write!(
                f,
                "{} of site {} is fixed by its site symmetry",
                axis.name(),
                site.0
            ),
            Self::CombinedCoordinate { site, axis } => write!(
                f,
                "{} of site {} is a combination of its free coordinates; set those instead",
                axis.name(),
                site.0
            ),
            Self::CoincidentSite { site } => write!(
                f,
                "the new site would coincide with an atom of site {}; add the species to that \
                 site as a mixed occupant instead",
                site.0
            ),
            Self::FixedCellParameter { param } => {
                write!(f, "{} is fixed by the symmetry", param_name(*param))
            }
            Self::LinkedCellParameters { param, other } => write!(
                f,
                "{} and {} are equal by symmetry but were given different values",
                param_name(*param),
                param_name(*other)
            ),
            Self::InvalidCell => f.write_str(
                "invalid cell: lengths must be positive, angles strictly between 0 and 180 \
                 degrees, and the metric tensor positive definite",
            ),
            Self::InvalidPosition => f.write_str("coordinates must be finite numbers"),
            Self::OccupancyOutOfRange { value } => {
                write!(f, "occupancy {value} is not in (0, 1]")
            }
            Self::LastOccupant { site } => write!(
                f,
                "site {} has only this occupant; remove the site instead",
                site.0
            ),
            Self::UnknownElement { symbol } => write!(f, "{symbol:?} is not an element symbol"),
            Self::InvalidLabel { label } => write!(
                f,
                "{label:?} is not a valid label: it must be non-empty, without white space, \
                 and not start with _ # $ ' \" [ ] or ;"
            ),
            Self::DuplicateLabel { label } => write!(f, "the label {label:?} is already in use"),
            Self::DuplicateSite(site) => write!(f, "a site with id {} already exists", site.0),
            Self::InvalidIndex { index } => write!(f, "index {index} is out of range"),
            Self::UnknownMeasurement => f.write_str("no such measurement"),
            Self::RepeatedAtom(atom) => write!(
                f,
                "the measurement names atom {} of site {} twice",
                atom.member, atom.site.0
            ),
            Self::AlreadyP1 => f.write_str("the structure is in P1 already"),
            Self::Transform(error) => error.fmt(f),
            Self::Symmetrize(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for EditError {}

/// What an edit request names: a whole site, or one atom of the crystal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Site(SiteId),
    Atom(AtomRef),
}

/// The sites an edit of `targets` acts on, in the order first named.
///
/// An atom stands for its site when the site has a single atom in the cell,
/// as every site in P1 does, or when the targets name every equivalent atom
/// of the site. Otherwise the edit would change part of an orbit and break
/// the symmetry; it is refused with [`EditError::BreaksSymmetry`], whose
/// guidance is to edit the whole site or to convert to P1 first (design
/// §7.1).
pub fn target_sites(
    structure: &Structure,
    derived: &Derived,
    targets: &[Target],
) -> Result<Vec<SiteId>, EditError> {
    let mut sites = Vec::new();
    let mut whole = BTreeSet::new();
    let mut members: BTreeMap<SiteId, BTreeSet<u16>> = BTreeMap::new();
    for target in targets {
        let site = match *target {
            Target::Site(site) => {
                structure.site(site).ok_or(EditError::UnknownSite(site))?;
                whole.insert(site);
                site
            }
            Target::Atom(atom) => {
                derived
                    .fractional(atom)
                    .ok_or(EditError::UnknownAtom(atom))?;
                members.entry(atom.site).or_default().insert(atom.member);
                atom.site
            }
        };
        if !sites.contains(&site) {
            sites.push(site);
        }
    }
    for (site, members) in members {
        let equivalent = derived.multiplicity(site).unwrap_or(1);
        if !whole.contains(&site) && members.len() < equivalent {
            return Err(EditError::BreaksSymmetry {
                site,
                atoms: members.len(),
                equivalent,
            });
        }
    }
    Ok(sites)
}

impl Edit {
    /// Applies the edit to a copy of `structure`. On error, nothing is
    /// applied, also for a batch whose later edit fails.
    pub fn apply(&self, structure: &Structure) -> Result<Applied, EditError> {
        let mut next = structure.clone();
        let mut report = Report::default();
        let (inverse, ids) = self.apply_to(&mut next, &mut report)?;
        Ok(Applied {
            structure: next,
            inverse,
            ids,
            warnings: report.warnings,
            info: report.info,
        })
    }

    fn apply_to(
        &self,
        structure: &mut Structure,
        report: &mut Report,
    ) -> Result<(Edit, IdMap), EditError> {
        let same = IdMap::default();
        Ok(match self {
            Self::SetCell { values } => {
                let params = constrained_cell(structure, values)?;
                set_cell(structure, params)?
            }
            Self::RestoreCell { params } => set_cell(structure, *params)?,
            Self::SetSpecies {
                site,
                occupant,
                species,
            } => {
                check_element(&species.element)?;
                let slot = occupant_mut(structure, *site, occupant)?;
                let old = std::mem::replace(&mut slot.species, species.clone());
                let inverse = Self::SetSpecies {
                    site: *site,
                    occupant: occupant.clone(),
                    species: old,
                };
                (inverse, same)
            }
            Self::SetOccupancy {
                site,
                occupant,
                occupancy,
            } => {
                check_occupancy(occupancy.value())?;
                let slot = occupant_mut(structure, *site, occupant)?;
                let old = std::mem::replace(&mut slot.occupancy, *occupancy);
                report
                    .warnings
                    .extend(occupancy_warning(site_ref(structure, *site)?));
                report
                    .warnings
                    .extend(split_occupancy_warning(structure, *site));
                let inverse = Self::SetOccupancy {
                    site: *site,
                    occupant: occupant.clone(),
                    occupancy: old,
                };
                (inverse, same)
            }
            Self::AddOccupant {
                site,
                species,
                occupancy,
                label,
            } => {
                check_element(&species.element)?;
                check_occupancy(occupancy.value())?;
                site_ref(structure, *site)?;
                let label = match label {
                    Some(label) => {
                        check_label(structure, label, None)?;
                        label.clone()
                    }
                    None => new_label(structure, &species.element),
                };
                let target = site_mut(structure, *site)?;
                target
                    .occupants
                    .push(Occupant::new(&label, species.clone(), *occupancy));
                report.warnings.extend(occupancy_warning(target));
                report
                    .warnings
                    .extend(split_occupancy_warning(structure, *site));
                let inverse = Self::RemoveOccupant {
                    site: *site,
                    occupant: label,
                };
                (inverse, same)
            }
            Self::RemoveOccupant { site, occupant } => {
                let target = site_mut(structure, *site)?;
                let index = occupant_index(target, occupant)?;
                if target.occupants.len() == 1 {
                    return Err(EditError::LastOccupant { site: *site });
                }
                let removed = target.occupants.remove(index);
                let inverse = Self::RestoreOccupant {
                    site: *site,
                    index,
                    occupant: removed,
                };
                (inverse, same)
            }
            Self::RestoreOccupant {
                site,
                index,
                occupant,
            } => {
                check_label(structure, &occupant.label, None)?;
                let target = site_mut(structure, *site)?;
                if *index > target.occupants.len() {
                    return Err(EditError::InvalidIndex { index: *index });
                }
                target.occupants.insert(*index, occupant.clone());
                let inverse = Self::RemoveOccupant {
                    site: *site,
                    occupant: occupant.label.clone(),
                };
                (inverse, same)
            }
            Self::RenameLabel {
                site,
                occupant,
                label,
            } => {
                let index = occupant_index(site_ref(structure, *site)?, occupant)?;
                check_label(structure, label, Some((*site, index)))?;
                site_mut(structure, *site)?.occupants[index].label = label.clone();
                let inverse = Self::RenameLabel {
                    site: *site,
                    occupant: label.clone(),
                    label: occupant.clone(),
                };
                (inverse, same)
            }
            Self::SetPosition { site, position } => {
                site_ref(structure, *site)?;
                check_position(position)?;
                let placed = placement::settle(structure, *position, 1);
                place(structure, *site, placed, report)?
            }
            Self::MoveSite {
                atom,
                displacement,
                leave_special_position,
            } => {
                check_position(displacement)?;
                let site = site_ref(structure, atom.site)?;
                let members = expand::members(structure, site);
                let &(op, _) = members
                    .get(usize::from(atom.member))
                    .ok_or(EditError::UnknownAtom(*atom))?;
                let placed = placement::moved(
                    structure,
                    site.position,
                    &structure.symmetry.operations()[op],
                    *displacement,
                    *leave_special_position,
                );
                place(structure, atom.site, placed, report)?
            }
            Self::SetCoordinate { site, axis, value } => {
                let current = site_ref(structure, *site)?.position;
                if !value.is_finite() {
                    return Err(EditError::InvalidPosition);
                }
                let placed = if (value - current[*axis as usize]).abs() <= SAME_VALUE {
                    Placement::at(current)
                } else {
                    placement::with_coordinate(structure, current, *axis, *value).map_err(
                        |constraint| match constraint {
                            CoordinateConstraint::Fixed => EditError::FixedCoordinate {
                                site: *site,
                                axis: *axis,
                            },
                            _ => EditError::CombinedCoordinate {
                                site: *site,
                                axis: *axis,
                            },
                        },
                    )?
                };
                place(structure, *site, placed, report)?
            }
            Self::RestorePosition {
                site,
                position,
                position_su,
            } => {
                check_position(position)?;
                relocate(structure, *site, *position, *position_su, report)?
            }
            Self::AddSite {
                position,
                species,
                occupancy,
                label,
            } => {
                check_position(position)?;
                check_element(&species.element)?;
                check_occupancy(occupancy.value())?;
                let label = match label {
                    Some(label) => {
                        check_label(structure, label, None)?;
                        label.clone()
                    }
                    None => new_label(structure, &species.element),
                };
                let special = special_position(structure, *position);
                let id = SiteId(structure.sites.iter().map(|s| s.id.0).max().unwrap_or(0) + 1);
                let occupant = Occupant::new(&label, species.clone(), *occupancy);
                structure
                    .sites
                    .push(Site::new(id, special.position, vec![occupant]));
                let orbits = expand::orbits(structure);
                let tolerance = structure.tolerances.position;
                let coincident = expand::contacts_with(structure, &orbits, id, tolerance)
                    .into_iter()
                    .min_by(|a, b| a.distance.total_cmp(&b.distance));
                if let Some(contact) = coincident {
                    return Err(EditError::CoincidentSite { site: contact.b });
                }
                if special.displacement >= REPORTED_SNAP {
                    report.info.push(EditInfo::Snapped {
                        site: id,
                        displacement: special.displacement,
                    });
                }
                report
                    .warnings
                    .extend(contact_warnings(structure, &orbits, id));
                (Self::RemoveSite { site: id }, same)
            }
            Self::RemoveSite { site } => {
                let index = site_index(structure, *site)?;
                let removed = structure.sites.remove(index);
                let inverse = Self::RestoreSite {
                    index,
                    site: removed,
                };
                (inverse, IdMap::removed(*site))
            }
            Self::RestoreSite { index, site } => {
                if structure.site(site.id).is_some() {
                    return Err(EditError::DuplicateSite(site.id));
                }
                if *index > structure.sites.len() {
                    return Err(EditError::InvalidIndex { index: *index });
                }
                for occupant in &site.occupants {
                    check_label(structure, &occupant.label, None)?;
                }
                structure.sites.insert(*index, site.clone());
                (Self::RemoveSite { site: site.id }, same)
            }
            Self::Batch { edits } => {
                let mut inverses = Vec::with_capacity(edits.len());
                let mut ids = IdMap::default();
                for edit in edits {
                    let (inverse, map) = edit.apply_to(structure, report)?;
                    inverses.push(inverse);
                    ids = ids.then(map);
                }
                inverses.reverse();
                (Self::Batch { edits: inverses }, ids)
            }
            Self::ExpandToP1 => {
                if structure.edit_mode() == EditMode::P1 {
                    return Err(EditError::AlreadyP1);
                }
                let expanded = transform::expand_to_p1(structure);
                let old = std::mem::replace(structure, expanded.structure);
                let ids = IdMap::with_fates(
                    expanded
                        .splits
                        .into_iter()
                        .map(|(site, sites)| (site, SiteFate::Split(sites))),
                );
                let inverse = Self::RestoreStructure {
                    structure: Box::new(old),
                };
                (inverse, ids)
            }
            Self::RestoreStructure {
                structure: restored,
            } => {
                let ids = restore_map(structure, restored);
                let old = std::mem::replace(structure, Structure::clone(restored));
                let inverse = Self::RestoreStructure {
                    structure: Box::new(old),
                };
                (inverse, ids)
            }
            Self::TransformCell { transform } => {
                // An exact inverse needs the old structure: coordinates
                // taken through P⁻¹ and back through P differ by rounding.
                let back = transform
                    .inverse()
                    .ok_or(EditError::Transform(TransformError::TooLarge))?;
                if transform.is_identity() {
                    let inverse = Self::RestoreBasis {
                        transform: back,
                        structure: Box::new(structure.clone()),
                    };
                    return Ok((inverse, same));
                }
                let changed =
                    transform::change_basis(structure, transform).map_err(EditError::Transform)?;
                let ids = transform::rebase_map(structure, &changed, transform);
                let old = std::mem::replace(structure, changed);
                let inverse = Self::RestoreBasis {
                    transform: back,
                    structure: Box::new(old),
                };
                (inverse, ids)
            }
            Self::RestoreBasis {
                transform,
                structure: restored,
            } => {
                let forward = transform
                    .inverse()
                    .ok_or(EditError::Transform(TransformError::TooLarge))?;
                let ids = transform::rebase_map(structure, restored, transform);
                let old = std::mem::replace(structure, Structure::clone(restored));
                let inverse = Self::RestoreBasis {
                    transform: forward,
                    structure: Box::new(old),
                };
                (inverse, ids)
            }
            Self::Symmetrize {
                symprec,
                standardize,
            } => {
                let detect = DetectSymmetry {
                    symprec: symprec.unwrap_or(structure.tolerances.symprec),
                    standardize: *standardize,
                };
                let result = detect.run(structure).map_err(EditError::Symmetrize)?;
                let ids = result.ids(structure);
                let back = result
                    .mapping
                    .inverse()
                    .ok_or(EditError::Symmetrize(SymmetrizeError::TooLarge))?;
                let old = std::mem::replace(structure, result.structure);
                let inverse = Self::RestoreAtoms {
                    mapping: back,
                    structure: Box::new(old),
                };
                (inverse, ids)
            }
            Self::RestoreAtoms {
                mapping,
                structure: restored,
            } => {
                let back = mapping
                    .inverse()
                    .ok_or(EditError::Symmetrize(SymmetrizeError::TooLarge))?;
                let ids = IdMap::relocated(structure, restored, *mapping);
                let old = std::mem::replace(structure, Structure::clone(restored));
                let inverse = Self::RestoreAtoms {
                    mapping: back,
                    structure: Box::new(old),
                };
                (inverse, ids)
            }
        })
    }
}

/// What a structure being replaced by `after` does to references: a site
/// that `after` has keeps them; one whose atom sits exactly where an atom
/// of `after` sits (a P1 site of an expansion being undone) becomes that
/// atom; the others are removed.
fn restore_map(before: &Structure, after: &Structure) -> IdMap {
    let key = |p: [f64; 3]| p.map(f64::to_bits);
    let mut atoms = std::collections::HashMap::new();
    for site in &after.sites {
        for (member, (_, position)) in expand::members(after, site).into_iter().enumerate() {
            atoms
                .entry(key(position))
                .or_insert((site.id, member as u16));
        }
    }
    let fates = before
        .sites
        .iter()
        .filter(|site| after.site(site.id).is_none())
        .map(|site| {
            let members = expand::members(before, site);
            let found = match members.as_slice() {
                [(_, position)] => atoms.get(&key(*position)),
                _ => None,
            };
            let fate = match found {
                Some(&(site, member)) => SiteFate::Merged { site, member },
                None => SiteFate::Removed,
            };
            (site.id, fate)
        });
    IdMap::with_fates(fates)
}

/// The edit that converts the structure to P1 and then removes what
/// `targets` name (design §7.1, "convert to P1, then delete this one"), as
/// one step: an atom removes the site it became, a whole site all of them.
pub fn expand_and_remove(structure: &Structure, targets: &[Target]) -> Result<Edit, EditError> {
    let expanded = Edit::ExpandToP1.apply(structure)?;
    let mut sites = Vec::new();
    for target in targets {
        let removed = match *target {
            Target::Site(site) => {
                structure.site(site).ok_or(EditError::UnknownSite(site))?;
                expanded.ids.sites(site)
            }
            Target::Atom(atom) => match expanded.ids.atom(atom) {
                AtomFate::Kept(kept) => vec![kept.site],
                AtomFate::Site(_) | AtomFate::Removed => {
                    return Err(EditError::UnknownAtom(atom));
                }
            },
        };
        for site in removed {
            if !sites.contains(&site) {
                sites.push(site);
            }
        }
    }
    let edits = std::iter::once(Edit::ExpandToP1)
        .chain(sites.into_iter().map(|site| Edit::RemoveSite { site }))
        .collect();
    Ok(Edit::Batch { edits })
}

/// The cell after setting `values` under the structure's cell constraints
/// (design §7.3): each value goes to the free parameter it is or follows,
/// and every parameter linked to that one takes it too. The result always
/// keeps the symmetry, so no values align a cell that does not.
fn constrained_cell(structure: &Structure, values: &CellValues) -> Result<CellParams, EditError> {
    let constraints = cell_constraints(&structure.symmetry);
    let old = *structure.lattice.params();
    let root = |param: CellParam| match constraints.get(param) {
        Constraint::Fixed => None,
        Constraint::Equals(root) => Some(root),
        Constraint::Free => Some(param),
    };
    // The cell the operations keep that is nearest the old one: its fixed
    // angles are the values the operations require.
    let ideal = CellParams::from_metric(symmetrized_metric(&structure.symmetry, old.metric()));
    // The value for each free parameter, and the parameter that gave it.
    let mut given: [Option<(CellParam, f64)>; 6] = [None; 6];
    for param in CellParam::ALL {
        let Some(value) = values.get(param) else {
            continue;
        };
        if !value.is_finite() {
            return Err(EditError::InvalidCell);
        }
        let Some(root) = root(param) else {
            if (value - ideal.get(param).value()).abs() > SAME_VALUE {
                return Err(EditError::FixedCellParameter { param });
            }
            continue;
        };
        match given[root as usize] {
            Some((other, v)) if (v - value).abs() > SAME_VALUE => {
                return Err(EditError::LinkedCellParameters { param, other });
            }
            Some(_) => {}
            None => given[root as usize] = Some((param, value)),
        }
    }
    // The result keeps the symmetry even where the old cell did not, as an
    // imported one may not quite (design §7.3): a fixed angle takes the
    // value the operations require, and a linked parameter left out takes
    // the old value of the one it follows.
    let mut params = old;
    for param in CellParam::ALL {
        let current = old.get(param);
        let Some(root) = root(param) else {
            let value = ideal.get(param).value();
            if (value - current.value()).abs() > SAME_VALUE {
                params.set(param, Measured::exact(value));
            }
            continue;
        };
        match given[root as usize] {
            // A value given again keeps its uncertainty.
            Some((_, value)) => {
                if value != current.value() {
                    params.set(param, Measured::exact(value));
                }
            }
            None => {
                let followed = old.get(root);
                if (followed.value() - current.value()).abs() > SAME_VALUE {
                    params.set(param, followed);
                }
            }
        }
    }
    Ok(params)
}

/// Sets the cell parameters. Fractional coordinates stay; a site whose
/// equivalent atoms merge or separate at the new distances is reshaped.
fn set_cell(structure: &mut Structure, params: CellParams) -> Result<(Edit, IdMap), EditError> {
    let lengths = [params.a, params.b, params.c].map(|m| m.value());
    let angles = [params.alpha, params.beta, params.gamma].map(|m| m.value());
    if !lengths.iter().chain(&angles).all(|v| v.is_finite()) || !params.is_valid() {
        return Err(EditError::InvalidCell);
    }
    let sizes = |structure: &Structure| {
        expand::orbits(structure)
            .into_iter()
            .map(|orbit| (orbit.site, orbit.positions.len()))
            .collect::<Vec<_>>()
    };
    let before = sizes(structure);
    let old = *structure.lattice.params();
    structure.lattice.set_params(params);
    let reshaped = before
        .into_iter()
        .zip(sizes(structure))
        .filter(|(before, after)| before != after)
        .map(|((site, _), _)| site);
    Ok((Edit::RestoreCell { params: old }, IdMap::reshaped(reshaped)))
}

/// Moves a site to where `placed` puts it. Coordinates that change lose
/// their uncertainty (design §4.5). Reports the snap, a change in the
/// number of equivalent atoms and atoms that come too close; a site that
/// stays where it is reports nothing.
fn place(
    structure: &mut Structure,
    site: SiteId,
    placed: Placement,
    report: &mut Report,
) -> Result<(Edit, IdMap), EditError> {
    let old = site_ref(structure, site)?;
    let su = [0, 1, 2].map(|i| {
        (placed.position[i] == old.position[i])
            .then_some(old.position_su[i])
            .flatten()
    });
    if placed.position == old.position {
        let inverse = Edit::RestorePosition {
            site,
            position: old.position,
            position_su: old.position_su,
        };
        return Ok((inverse, IdMap::default()));
    }
    if placed.snapped >= REPORTED_SNAP {
        report.info.push(EditInfo::Snapped {
            site,
            displacement: placed.snapped,
        });
    }
    let moved = relocate(structure, site, placed.position, su, report)?;
    let orbits = expand::orbits(structure);
    report
        .warnings
        .extend(contact_warnings(structure, &orbits, site));
    Ok(moved)
}

/// Puts a site at `position` with these uncertainties. Reports a change in
/// its number of equivalent atoms. References to its atoms keep their
/// member numbers only while the orbit's members come from the same
/// operations as before (design §7.11); otherwise they keep the site.
fn relocate(
    structure: &mut Structure,
    site: SiteId,
    position: [f64; 3],
    position_su: [Option<f64>; 3],
    report: &mut Report,
) -> Result<(Edit, IdMap), EditError> {
    let index = site_index(structure, site)?;
    let operations = |structure: &Structure| -> Vec<usize> {
        expand::members(structure, &structure.sites[index])
            .into_iter()
            .map(|(op, _)| op)
            .collect()
    };
    let before = operations(structure);
    let target = &mut structure.sites[index];
    let inverse = Edit::RestorePosition {
        site,
        position: std::mem::replace(&mut target.position, position),
        position_su: std::mem::replace(&mut target.position_su, position_su),
    };
    let after = operations(structure);
    if before.len() != after.len() {
        report.info.push(EditInfo::Multiplicity {
            site,
            before: before.len(),
            after: after.len(),
        });
    }
    let ids = if before == after {
        IdMap::default()
    } else {
        IdMap::reshaped([site])
    };
    Ok((inverse, ids))
}

/// Atoms of `site` closer than the short-contact distance to atoms of other
/// sites, or to each other (design §7.4).
fn contact_warnings(
    structure: &Structure,
    orbits: &[expand::Orbit],
    site: SiteId,
) -> Vec<EditWarning> {
    let short = structure.tolerances.short_contact;
    let images = orbits
        .iter()
        .find(|orbit| orbit.site == site)
        .and_then(|orbit| orbit.nearest_image)
        .filter(|distance| *distance < short)
        .map(|distance| EditWarning::CloseImages { site, distance });
    images
        .into_iter()
        .chain(
            expand::contacts_with(structure, orbits, site, short)
                .into_iter()
                .map(|contact| EditWarning::ShortContact {
                    site,
                    other: contact.b,
                    distance: contact.distance,
                }),
        )
        .collect()
}

fn check_position(position: &[f64; 3]) -> Result<(), EditError> {
    if position.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(EditError::InvalidPosition)
    }
}

fn site_index(structure: &Structure, site: SiteId) -> Result<usize, EditError> {
    structure
        .sites
        .iter()
        .position(|s| s.id == site)
        .ok_or(EditError::UnknownSite(site))
}

fn site_ref(structure: &Structure, site: SiteId) -> Result<&Site, EditError> {
    structure.site(site).ok_or(EditError::UnknownSite(site))
}

fn site_mut(structure: &mut Structure, site: SiteId) -> Result<&mut Site, EditError> {
    let index = site_index(structure, site)?;
    Ok(&mut structure.sites[index])
}

fn occupant_index(site: &Site, label: &str) -> Result<usize, EditError> {
    site.occupants
        .iter()
        .position(|o| o.label == label)
        .ok_or_else(|| EditError::UnknownOccupant {
            site: site.id,
            label: label.to_string(),
        })
}

fn occupant_mut<'a>(
    structure: &'a mut Structure,
    site: SiteId,
    label: &str,
) -> Result<&'a mut Occupant, EditError> {
    let site = site_mut(structure, site)?;
    let index = occupant_index(site, label)?;
    Ok(&mut site.occupants[index])
}

fn check_element(symbol: &str) -> Result<(), EditError> {
    if element::is_element(symbol) {
        Ok(())
    } else {
        Err(EditError::UnknownElement {
            symbol: symbol.to_string(),
        })
    }
}

fn check_occupancy(value: f64) -> Result<(), EditError> {
    if value.is_finite() && value > 0.0 && value <= 1.0 {
        Ok(())
    } else {
        Err(EditError::OccupancyOutOfRange { value })
    }
}

/// A label must be usable in CIF and unique within the structure, apart
/// from the occupant `except` that already carries it.
fn check_label(
    structure: &Structure,
    label: &str,
    except: Option<(SiteId, usize)>,
) -> Result<(), EditError> {
    let reserved = ['_', '#', '$', '\'', '"', '[', ']', ';'];
    if label.is_empty() || label.contains(char::is_whitespace) || label.starts_with(reserved) {
        return Err(EditError::InvalidLabel {
            label: label.to_string(),
        });
    }
    let taken = structure.sites.iter().any(|site| {
        site.occupants
            .iter()
            .enumerate()
            .any(|(index, o)| o.label == label && except != Some((site.id, index)))
    });
    if taken {
        return Err(EditError::DuplicateLabel {
            label: label.to_string(),
        });
    }
    Ok(())
}

/// The element symbol and the smallest number not yet in use, e.g. `Ni2`.
fn new_label(structure: &Structure, element: &str) -> String {
    let taken: BTreeSet<&str> = structure
        .sites
        .iter()
        .flat_map(|site| site.occupants.iter().map(|o| o.label.as_str()))
        .collect();
    (1..)
        .map(|n| format!("{element}{n}"))
        .find(|label| !taken.contains(label.as_str()))
        .expect("finitely many labels")
}

fn occupancy_sum(site: &Site) -> f64 {
    site.occupants.iter().map(|o| o.occupancy.value()).sum()
}

fn occupancy_warning(site: &Site) -> Option<EditWarning> {
    let sum = occupancy_sum(site);
    (sum > 1.0 + OCCUPANCY_TOLERANCE).then_some(EditWarning::OccupancySum { site: site.id, sum })
}

/// Split positions of one disorder assembly are never occupied together,
/// so a split site and the alternatives next to it may hold one atom
/// between them (design §7.7). Alternatives are sites of the other groups
/// of the assembly, as for bonds; next to it means an atom closer than the
/// two elements' covalent radii, nearer than a bond would put them, or
/// than the short-contact distance for elements without a radius. Of each
/// other group the adjacent site with the largest occupancy counts. A site
/// that alone holds more than 1 has its own warning.
fn split_occupancy_warning(structure: &Structure, site: SiteId) -> Option<EditWarning> {
    let own = structure.site(site)?;
    own.disorder.as_ref()?;
    let own_sum = occupancy_sum(own);
    if own_sum > 1.0 + OCCUPANCY_TOLERANCE {
        return None;
    }
    let short = structure.tolerances.short_contact;
    let limit =
        |a: &Site, b: &Site| match (covalent_radius(a.element()), covalent_radius(b.element())) {
            (Some(ra), Some(rb)) => ra + rb,
            _ => short,
        };
    let candidates: Vec<&Site> = structure
        .sites
        .iter()
        .filter(|other| alternatives(own, other))
        .collect();
    let reach = candidates
        .iter()
        .map(|other| limit(own, other))
        .fold(0.0, f64::max);
    if reach <= 0.0 {
        return None;
    }
    let orbits = expand::orbits(structure);
    // For each other group, the adjacent site with the largest occupancy.
    let mut groups: BTreeMap<&str, (f64, SiteId)> = BTreeMap::new();
    for contact in expand::contacts_with(structure, &orbits, site, reach) {
        let Some(other) = candidates.iter().find(|s| s.id == contact.b) else {
            continue;
        };
        if contact.distance >= limit(own, other) {
            continue;
        }
        let group = other.disorder.as_ref().map_or("", |d| d.group.as_str());
        let occupancy = occupancy_sum(other);
        let best = groups.entry(group).or_insert((occupancy, other.id));
        if occupancy > best.0 {
            *best = (occupancy, other.id);
        }
    }
    if groups.is_empty() {
        return None;
    }
    let sum = own_sum + groups.values().map(|(occupancy, _)| occupancy).sum::<f64>();
    let mut partners: Vec<SiteId> = groups.values().map(|&(_, id)| id).collect();
    partners.sort();
    (sum > 1.0 + OCCUPANCY_TOLERANCE).then_some(EditWarning::SplitOccupancySum {
        site,
        partners,
        sum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{self, SampleKey};
    use crate::{Adp, AtomFate, CellParams, Disorder, Lattice, SymOp, Symmetry};

    fn load(key: SampleKey) -> (Structure, Derived) {
        let sample = sample::load(key);
        (sample.structure, sample.derived)
    }

    fn site(structure: &Structure, label: &str) -> SiteId {
        structure
            .sites
            .iter()
            .find(|s| s.label() == label)
            .expect("site")
            .id
    }

    /// Applies the edit, then its inverse, and checks that the inverse
    /// restores the structure exactly.
    fn round_trip(structure: &Structure, edit: Edit) -> Applied {
        let applied = edit.apply(structure).expect("edit applies");
        let restored = applied
            .inverse
            .apply(&applied.structure)
            .expect("inverse applies");
        assert_eq!(&restored.structure, structure, "{edit:?}");
        assert_eq!(
            restored.inverse.apply(structure).unwrap().structure,
            applied.structure,
            "the inverse of the inverse redoes {edit:?}"
        );
        applied
    }

    fn cell(structure: &Structure) -> [f64; 6] {
        CellParam::ALL.map(|p| structure.lattice.params().get(p).value())
    }

    #[test]
    fn linked_cell_parameters_follow() {
        let (rutile, _) = load(SampleKey::Rutile);
        let a = round_trip(
            &rutile,
            Edit::SetCell {
                values: CellValues::default().with(CellParam::A, 4.7),
            },
        );
        assert_eq!(cell(&a.structure), [4.7, 4.7, 2.959, 90.0, 90.0, 90.0]);
        assert!(a.ids.is_identity());
        // A follower given alone moves the parameter it follows.
        let b = Edit::SetCell {
            values: CellValues::default().with(CellParam::B, 4.8),
        };
        let b = b.apply(&rutile).unwrap();
        assert_eq!(cell(&b.structure)[..3], [4.8, 4.8, 2.959]);

        let (srtio3, _) = load(SampleKey::StrontiumTitanate);
        let c = Edit::SetCell {
            values: CellValues::default().with(CellParam::C, 4.0),
        };
        assert_eq!(
            cell(&c.apply(&srtio3).unwrap().structure),
            [4.0, 4.0, 4.0, 90.0, 90.0, 90.0]
        );
    }

    #[test]
    fn rhombohedral_angles_change_together() {
        let generators: Vec<SymOp> = ["z,x,y", "-y,-x,-z", "-x,-y,-z"]
            .iter()
            .map(|g| SymOp::parse(g).unwrap())
            .collect();
        let structure = Structure::new(
            Lattice::new(CellParams::new(5.0, 5.0, 5.0, 60.0, 60.0, 60.0)),
            Symmetry::generate(&generators).unwrap(),
            vec![Site::single(1, "Si1", "Si", [0.0; 3])],
        );
        let edit = Edit::SetCell {
            values: CellValues::default().with(CellParam::Gamma, 70.0),
        };
        let applied = round_trip(&structure, edit);
        assert_eq!(cell(&applied.structure), [5.0, 5.0, 5.0, 70.0, 70.0, 70.0]);
    }

    #[test]
    fn constrained_cell_edits_are_refused() {
        let (rutile, _) = load(SampleKey::Rutile);
        let set = |values: CellValues| Edit::SetCell { values }.apply(&rutile).err();
        assert_eq!(
            set(CellValues::default().with(CellParam::Alpha, 95.0)),
            Some(EditError::FixedCellParameter {
                param: CellParam::Alpha
            })
        );
        // Giving a fixed angle its value is fine.
        assert_eq!(
            set(CellValues::default().with(CellParam::Gamma, 90.0)),
            None
        );
        assert_eq!(
            set(CellValues::default()
                .with(CellParam::A, 4.0)
                .with(CellParam::B, 4.1)),
            Some(EditError::LinkedCellParameters {
                param: CellParam::B,
                other: CellParam::A
            })
        );
        assert_eq!(
            set(CellValues::default().with(CellParam::C, -1.0)),
            Some(EditError::InvalidCell)
        );
        assert_eq!(
            set(CellValues::default().with(CellParam::C, f64::NAN)),
            Some(EditError::InvalidCell)
        );
    }

    #[test]
    fn cell_edits_align_a_cell_off_its_symmetry() {
        // An imported cubic cell a little off: b and c written to other
        // decimals, α off 90° (design §7.3).
        let (mut srtio3, _) = load(SampleKey::StrontiumTitanate);
        let off = CellParams::new(3.905, 3.9051, 3.9049, 90.01, 90.0, 90.0);
        srtio3.lattice.set_params(off);
        assert!(crate::cell_deviation(&srtio3.symmetry, &off).0 > 1e-5);

        // No values align it: b and c follow a, α takes 90°. One step,
        // undone exactly.
        let aligned = round_trip(
            &srtio3,
            Edit::SetCell {
                values: CellValues::default(),
            },
        );
        let params = *aligned.structure.lattice.params();
        assert_eq!(params.a, off.a);
        assert_eq!(params.b, off.a);
        assert_eq!(params.c, off.a);
        assert_eq!(params.alpha, Measured::exact(90.0));
        let (length, angle) = crate::cell_deviation(&aligned.structure.symmetry, &params);
        assert!(length < 1e-12 && angle < 1e-12, "{length} Å, {angle}°");
        // Giving a fixed angle the value the symmetry requires is fine; any
        // other edit aligns the cell as well.
        let edited = Edit::SetCell {
            values: CellValues::default()
                .with(CellParam::Alpha, 90.0)
                .with(CellParam::A, 3.91),
        }
        .apply(&srtio3)
        .unwrap();
        assert_eq!(
            cell(&edited.structure),
            [3.91, 3.91, 3.91, 90.0, 90.0, 90.0]
        );

        // A cell that keeps its symmetry stays as it is.
        let (rutile, _) = load(SampleKey::Rutile);
        let same = Edit::SetCell {
            values: CellValues::default(),
        }
        .apply(&rutile)
        .unwrap();
        assert_eq!(same.structure, rutile);
    }

    #[test]
    fn p1_cells_are_free_but_must_be_valid() {
        let (supercell, _) = load(SampleKey::RutileSupercell);
        let skewed = CellValues::default()
            .with(CellParam::Alpha, 80.0)
            .with(CellParam::B, 9.0);
        let applied = round_trip(&supercell, Edit::SetCell { values: skewed });
        assert_eq!(cell(&applied.structure)[1..4], [9.0, 5.918, 80.0]);
        let impossible = CellValues::default()
            .with(CellParam::Alpha, 10.0)
            .with(CellParam::Beta, 10.0)
            .with(CellParam::Gamma, 170.0);
        assert_eq!(
            Edit::SetCell { values: impossible }.apply(&supercell).err(),
            Some(EditError::InvalidCell)
        );
    }

    #[test]
    fn typed_values_drop_the_uncertainty_and_undo_restores_it() {
        let mut structure = load(SampleKey::Rutile).0;
        let mut params = *structure.lattice.params();
        params.a = Measured::with_su(4.594, 0.002);
        params.b = Measured::with_su(4.594, 0.002);
        params.c = Measured::with_su(2.959, 0.001);
        structure.lattice.set_params(params);
        let edit = Edit::SetCell {
            values: CellValues::default()
                .with(CellParam::A, 4.6)
                .with(CellParam::C, 2.959),
        };
        let applied = round_trip(&structure, edit);
        let params = applied.structure.lattice.params();
        assert_eq!(params.a, Measured::exact(4.6));
        assert_eq!(params.b, Measured::exact(4.6));
        // Given again unchanged: keeps its uncertainty.
        assert_eq!(params.c, Measured::with_su(2.959, 0.001));
    }

    #[test]
    fn atoms_that_separate_reshape_their_site() {
        // A mirror at z = 0 and an atom 0.001 Å from it: one atom in a
        // short cell, two once the cell is ten times longer.
        let structure = Structure::new(
            Lattice::new(CellParams::new(4.0, 4.0, 4.0, 90.0, 90.0, 90.0)),
            Symmetry::generate(&[SymOp::parse("x,y,-z").unwrap()]).unwrap(),
            vec![
                Site::single(1, "O1", "O", [0.1, 0.2, 0.00025]),
                Site::single(2, "O2", "O", [0.5, 0.5, 0.3]),
            ],
        );
        let edit = Edit::SetCell {
            values: CellValues::default().with(CellParam::C, 40.0),
        };
        let applied = round_trip(&structure, edit);
        let atom = AtomRef::new(SiteId(1), 0, [0; 3]);
        assert_eq!(applied.ids.atom(atom), AtomFate::Site(SiteId(1)));
        let other = AtomRef::new(SiteId(2), 1, [0; 3]);
        assert_eq!(applied.ids.atom(other), AtomFate::Kept(other));
    }

    #[test]
    fn species_change_on_the_whole_orbit_and_keep_their_label() {
        let (rutile, _) = load(SampleKey::Rutile);
        let ti = site(&rutile, "Ti1");
        let applied = round_trip(
            &rutile,
            Edit::SetSpecies {
                site: ti,
                occupant: "Ti1".into(),
                species: Species::element("Zr"),
            },
        );
        let site = applied.structure.site(ti).unwrap();
        assert_eq!((site.label(), site.element()), ("Ti1", "Zr"));
        let derived = Derived::compute(&applied.structure);
        assert_eq!(derived.multiplicity(ti), Some(2));

        let unknown = Edit::SetSpecies {
            site: ti,
            occupant: "Ti1".into(),
            species: Species::element("Zz"),
        };
        assert_eq!(
            unknown.apply(&rutile).err(),
            Some(EditError::UnknownElement {
                symbol: "Zz".into()
            })
        );
        let missing = Edit::SetSpecies {
            site: ti,
            occupant: "O1".into(),
            species: Species::element("Zr"),
        };
        assert!(matches!(
            missing.apply(&rutile),
            Err(EditError::UnknownOccupant { .. })
        ));
    }

    #[test]
    fn mixed_occupancy() {
        let (rutile, _) = load(SampleKey::Rutile);
        let ti = site(&rutile, "Ti1");
        let add = Edit::AddOccupant {
            site: ti,
            species: Species::element("Zr"),
            occupancy: Measured::exact(0.1),
            label: None,
        };
        let added = round_trip(&rutile, add.clone());
        assert_eq!(
            added.warnings,
            [EditWarning::OccupancySum { site: ti, sum: 1.1 }]
        );
        let occupants = &added.structure.site(ti).unwrap().occupants;
        assert_eq!(occupants[1].label, "Zr1");
        // The next one gets the next free number.
        let again = add.apply(&added.structure).unwrap();
        assert_eq!(again.structure.site(ti).unwrap().occupants[2].label, "Zr2");

        let lower = Edit::SetOccupancy {
            site: ti,
            occupant: "Ti1".into(),
            occupancy: Measured::exact(0.9),
        };
        let lowered = round_trip(&added.structure, lower);
        assert!(lowered.warnings.is_empty());

        for value in [0.0, -0.5, 1.2, f64::INFINITY] {
            let edit = Edit::SetOccupancy {
                site: ti,
                occupant: "Ti1".into(),
                occupancy: Measured::exact(value),
            };
            assert!(matches!(
                edit.apply(&rutile),
                Err(EditError::OccupancyOutOfRange { .. })
            ));
        }

        let taken = Edit::AddOccupant {
            site: ti,
            species: Species::element("Zr"),
            occupancy: Measured::exact(0.1),
            label: Some("O1".into()),
        };
        assert_eq!(
            taken.apply(&rutile).err(),
            Some(EditError::DuplicateLabel { label: "O1".into() })
        );
    }

    #[test]
    fn removing_occupants_restores_them_in_place() {
        let (rutile, _) = load(SampleKey::Rutile);
        let ti = site(&rutile, "Ti1");
        let mut structure = rutile.clone();
        let mixed = &mut structure.sites[0].occupants;
        mixed[0].occupancy = Measured::with_su(0.7, 0.01);
        mixed[0].adp = Some(Adp::Isotropic(Measured::with_su(0.006, 0.001)));
        mixed.push(Occupant::new(
            "Zr1",
            Species::new("Zr", Some(4), "Zr4+"),
            Measured::exact(0.3),
        ));
        let removed = round_trip(
            &structure,
            Edit::RemoveOccupant {
                site: ti,
                occupant: "Ti1".into(),
            },
        );
        assert_eq!(removed.structure.site(ti).unwrap().label(), "Zr1");
        assert_eq!(
            Edit::RemoveOccupant {
                site: ti,
                occupant: "Ti1".into()
            }
            .apply(&rutile)
            .err(),
            Some(EditError::LastOccupant { site: ti })
        );
        assert_eq!(
            EditError::LastOccupant { site: ti }.guidance(),
            [Guidance::RemoveSite { site: ti }]
        );
    }

    #[test]
    fn labels() {
        let (rutile, _) = load(SampleKey::Rutile);
        let o = site(&rutile, "O1");
        let rename = |label: &str| Edit::RenameLabel {
            site: o,
            occupant: "O1".into(),
            label: label.into(),
        };
        let applied = round_trip(&rutile, rename("O2"));
        assert_eq!(applied.structure.site(o).unwrap().label(), "O2");
        // Renaming to its own label is allowed and changes nothing.
        assert_eq!(rename("O1").apply(&rutile).unwrap().structure, rutile);
        for bad in ["", "O 2", "_O", "#1"] {
            assert!(matches!(
                rename(bad).apply(&rutile),
                Err(EditError::InvalidLabel { .. })
            ));
        }
        assert!(matches!(
            rename("Ti1").apply(&rutile),
            Err(EditError::DuplicateLabel { .. })
        ));
    }

    #[test]
    fn split_sites_next_to_each_other_hold_one_atom() {
        // Split positions of assembly A in a 10 Å P1 cell: C1B (group 2)
        // 0.1 Å from C1A (group 1) and C1C (group 3); C2A, in group 1 as
        // C1A, is 0.2 Å away, and of group 1 only the larger counts. Next
        // to it but not alternatives: the ordered C0 (group 0) and C1D of
        // assembly B. C3A of group 1 is far away.
        let disorder =
            |assembly: &str, group: &str| Disorder::new(Some(assembly.into()), group.into());
        let split = |id: u32, label: &str, position: [f64; 3], occupancy: f64, d: Disorder| {
            let occupant = Occupant::new(label, Species::element("C"), Measured::exact(occupancy));
            Site::new(SiteId(id), position, vec![occupant]).with_disorder(d)
        };
        let structure = Structure::new(
            Lattice::new(CellParams::new(10.0, 10.0, 10.0, 90.0, 90.0, 90.0)),
            Symmetry::p1(),
            vec![
                split(1, "C1A", [0.10, 0.10, 0.10], 0.5, disorder("A", "1")),
                split(2, "C2A", [0.09, 0.10, 0.10], 0.4, disorder("A", "1")),
                split(3, "C1B", [0.11, 0.10, 0.10], 0.3, disorder("A", "2")),
                split(4, "C1C", [0.11, 0.11, 0.10], 0.2, disorder("A", "3")),
                split(5, "C0", [0.11, 0.10, 0.11], 1.0, disorder("A", "0")),
                split(6, "C1D", [0.11, 0.09, 0.10], 0.9, disorder("B", "1")),
                split(7, "C3A", [0.5, 0.5, 0.5], 0.9, disorder("A", "1")),
            ],
        );
        let c1b = SiteId(3);
        let set = |structure: &Structure, occupancy: f64| {
            round_trip(
                structure,
                Edit::SetOccupancy {
                    site: c1b,
                    occupant: "C1B".into(),
                    occupancy: Measured::exact(occupancy),
                },
            )
        };
        let split_sum = |applied: &Applied| match &applied.warnings[..] {
            [
                EditWarning::SplitOccupancySum {
                    site,
                    partners,
                    sum,
                },
            ] if *site == c1b => Some((partners.clone(), *sum)),
            [] => None,
            other => panic!("unexpected warnings {other:?}"),
        };

        // 0.3 + 0.5 of group 1 (the larger of C1A and C2A) + 0.2 of group 3.
        assert_eq!(split_sum(&set(&structure, 0.3)), None);
        let (partners, sum) = split_sum(&set(&structure, 0.4)).expect("a warning");
        assert_eq!(partners, [SiteId(1), SiteId(4)]);
        assert!((sum - 1.1).abs() < 1e-12, "{sum}");
        // Within the tolerance of refinements.
        assert_eq!(split_sum(&set(&structure, 0.305)), None);

        // Adding an occupant to the split site counts as well.
        let added = round_trip(
            &structure,
            Edit::AddOccupant {
                site: c1b,
                species: Species::element("N"),
                occupancy: Measured::exact(0.2),
                label: None,
            },
        );
        let (partners, sum) = split_sum(&added).expect("a warning");
        assert_eq!(partners, [SiteId(1), SiteId(4)]);
        assert!((sum - 1.2).abs() < 1e-12, "{sum}");

        // A site that alone holds more than one atom has its own warning.
        let full = set(&structure, 1.0).structure;
        let over = Edit::AddOccupant {
            site: c1b,
            species: Species::element("N"),
            occupancy: Measured::exact(0.1),
            label: None,
        }
        .apply(&full)
        .unwrap();
        assert!(matches!(
            over.warnings[..],
            [EditWarning::OccupancySum { site, .. }] if site == c1b
        ));

        // The ordered part (group 0) is checked on its own only.
        let ordered = Edit::SetOccupancy {
            site: SiteId(5),
            occupant: "C0".into(),
            occupancy: Measured::exact(0.9),
        };
        assert!(ordered.apply(&structure).unwrap().warnings.is_empty());
    }

    #[test]
    fn removed_sites_come_back_in_place() {
        let (srtio3, _) = load(SampleKey::StrontiumTitanate);
        let ti = site(&srtio3, "Ti1");
        let applied = round_trip(&srtio3, Edit::RemoveSite { site: ti });
        assert_eq!(applied.structure.sites.len(), 2);
        assert_eq!(applied.ids.sites(ti), []);
        assert_eq!(
            applied.ids.atom(AtomRef::new(ti, 0, [0; 3])),
            AtomFate::Removed
        );
        assert_eq!(
            Edit::RemoveSite { site: ti }
                .apply(&applied.structure)
                .err(),
            Some(EditError::UnknownSite(ti))
        );
        // Restoring a site that exists is refused.
        let restore = applied.inverse.clone();
        assert_eq!(
            restore.apply(&srtio3).err(),
            Some(EditError::DuplicateSite(ti))
        );
    }

    #[test]
    fn batches_apply_all_or_nothing() {
        let (srtio3, _) = load(SampleKey::StrontiumTitanate);
        let [sr, ti, o] = [0, 1, 2].map(|i| srtio3.sites[i].id);
        let both = Edit::Batch {
            edits: vec![Edit::RemoveSite { site: sr }, Edit::RemoveSite { site: o }],
        };
        let applied = round_trip(&srtio3, both);
        assert_eq!(applied.structure.sites.len(), 1);
        assert_eq!(applied.structure.sites[0].id, ti);
        assert!(applied.ids.sites(sr).is_empty() && applied.ids.sites(o).is_empty());

        let failing = Edit::Batch {
            edits: vec![Edit::RemoveSite { site: sr }, Edit::RemoveSite { site: sr }],
        };
        assert_eq!(
            failing.apply(&srtio3).err(),
            Some(EditError::UnknownSite(sr))
        );
    }

    #[test]
    fn moving_atoms_in_p1() {
        let (supercell, _) = load(SampleKey::RutileSupercell);
        let first = supercell.sites[0].id;
        let applied = round_trip(
            &supercell,
            Edit::SetPosition {
                site: first,
                position: [0.01, 0.02, 0.03],
            },
        );
        assert!(applied.warnings.is_empty());
        // Onto another atom: allowed, with a warning.
        let other = supercell.sites[1].clone();
        let onto = Edit::SetPosition {
            site: first,
            position: other.position,
        };
        let applied = onto.apply(&supercell).unwrap();
        assert!(matches!(
            applied.warnings[..],
            [EditWarning::ShortContact { site, other: o, distance }]
                if site == first && o == other.id && distance < 1e-9
        ));
        assert_eq!(
            Edit::SetPosition {
                site: first,
                position: [f64::NAN, 0.0, 0.0]
            }
            .apply(&supercell)
            .err(),
            Some(EditError::InvalidPosition)
        );
    }

    #[test]
    fn single_atoms_under_symmetry_are_refused() {
        let (srtio3, derived) = load(SampleKey::StrontiumTitanate);
        let [sr, ti, o] = [0, 1, 2].map(|i| srtio3.sites[i].id);
        let atom = |site, member| Target::Atom(AtomRef::new(site, member, [0; 3]));
        let resolve = |targets: &[Target]| target_sites(&srtio3, &derived, targets);

        // One of the three oxygens.
        let error = resolve(&[atom(o, 1)]).unwrap_err();
        assert_eq!(
            error,
            EditError::BreaksSymmetry {
                site: o,
                atoms: 1,
                equivalent: 3
            }
        );
        assert_eq!(
            error.guidance(),
            [Guidance::WholeSite { site: o }, Guidance::ConvertToP1]
        );
        // All three, or the site itself, is the whole orbit.
        assert_eq!(resolve(&[atom(o, 0), atom(o, 1), atom(o, 2)]), Ok(vec![o]));
        assert_eq!(resolve(&[Target::Site(o), atom(o, 1)]), Ok(vec![o]));
        // Sr and Ti are alone in their orbits, whatever the cell.
        let image = Target::Atom(AtomRef::new(sr, 0, [1, 0, 0]));
        assert_eq!(
            resolve(&[atom(ti, 0), image, atom(sr, 0)]),
            Ok(vec![ti, sr])
        );

        assert_eq!(
            resolve(&[atom(o, 3)]),
            Err(EditError::UnknownAtom(AtomRef::new(o, 3, [0; 3])))
        );
        assert_eq!(
            resolve(&[Target::Site(SiteId(99))]),
            Err(EditError::UnknownSite(SiteId(99)))
        );
    }

    #[test]
    fn every_atom_in_p1_is_its_own_site() {
        let (supercell, derived) = load(SampleKey::RutileSupercell);
        // The first displayed atoms are a corner atom and its images.
        let targets: Vec<Target> = derived.atoms[..3]
            .iter()
            .chain(derived.atoms.iter().filter(|a| a.atom.site == SiteId(5)))
            .map(|atom| Target::Atom(atom.atom))
            .collect();
        let sites = target_sites(&supercell, &derived, &targets).unwrap();
        assert_eq!(sites, [SiteId(1), SiteId(5)]);
    }

    #[test]
    fn converting_to_p1_is_one_undoable_step() {
        let (rutile, derived) = load(SampleKey::Rutile);
        let applied = round_trip(&rutile, Edit::ExpandToP1);
        assert!(applied.structure.symmetry.is_p1());
        assert_eq!(applied.structure.sites.len(), 6);
        // Every atom follows to the site it became, in the same cell.
        let expanded = Derived::compute(&applied.structure);
        for atom in &derived.atoms {
            let AtomFate::Kept(kept) = applied.ids.atom(atom.atom) else {
                panic!("{:?} lost", atom.atom);
            };
            assert_eq!(kept.member, 0);
            let before = derived.fractional(atom.atom).unwrap();
            let after = expanded.fractional(kept).unwrap();
            assert_eq!(before, after);
        }
        // And back: the P1 atoms become members of the symmetric sites again.
        let undone = applied.inverse.apply(&applied.structure).unwrap();
        for atom in &derived.atoms {
            let AtomFate::Kept(kept) = applied.ids.atom(atom.atom) else {
                unreachable!()
            };
            assert_eq!(undone.ids.atom(kept), AtomFate::Kept(atom.atom));
        }
        assert_eq!(
            Edit::ExpandToP1.apply(&applied.structure).unwrap_err(),
            EditError::AlreadyP1
        );
    }

    #[test]
    fn converting_to_p1_then_deleting_one_atom() {
        let (rutile, derived) = load(SampleKey::Rutile);
        let o1 = site(&rutile, "O1");
        let one = derived
            .atoms
            .iter()
            .map(|a| a.atom)
            .find(|a| a.site == o1 && a.member == 2)
            .unwrap();
        let edit = expand_and_remove(&rutile, &[Target::Atom(one)]).unwrap();
        let applied = round_trip(&rutile, edit);
        assert_eq!(applied.structure.sites.len(), 5);
        let labels: Vec<&str> = applied.structure.sites.iter().map(Site::label).collect();
        assert_eq!(labels, ["Ti1_1", "Ti1_2", "O1_1", "O1_2", "O1_4"]);
        assert_eq!(applied.ids.atom(one), AtomFate::Removed);
        // A whole site removes all the sites it became.
        let ti = site(&rutile, "Ti1");
        let edit = expand_and_remove(&rutile, &[Target::Site(ti)]).unwrap();
        let applied = round_trip(&rutile, edit);
        assert!(applied.structure.sites.iter().all(|s| s.element() == "O"));
        assert_eq!(
            expand_and_remove(&rutile, &[Target::Atom(AtomRef::new(o1, 9, [0; 3]))]).unwrap_err(),
            EditError::UnknownAtom(AtomRef::new(o1, 9, [0; 3]))
        );
    }

    #[test]
    fn edits_serialize_as_tagged_json() {
        let edits = [
            Edit::SetCell {
                values: CellValues::default().with(CellParam::A, 5.0),
            },
            Edit::SetSpecies {
                site: SiteId(1),
                occupant: "Ti1".into(),
                species: Species::new("Fe", Some(3), "Fe3+"),
            },
            Edit::AddOccupant {
                site: SiteId(1),
                species: Species::element("Ni"),
                occupancy: Measured::with_su(0.1, 0.01),
                label: None,
            },
            Edit::Batch {
                edits: vec![
                    Edit::RemoveSite { site: SiteId(2) },
                    Edit::RestoreSite {
                        index: 0,
                        site: Site::single(3, "O1", "O", [0.5, 0.0, 0.25]),
                    },
                ],
            },
        ];
        for edit in edits {
            let json = serde_json::to_string(&edit).unwrap();
            assert_eq!(serde_json::from_str::<Edit>(&json).unwrap(), edit, "{json}");
        }
        let json = serde_json::to_value(Edit::ExpandToP1).unwrap();
        assert_eq!(json, serde_json::json!({"command": "expand_to_p1"}));
        let (rutile, _) = load(SampleKey::Rutile);
        let restore = Edit::RestoreStructure {
            structure: Box::new(rutile),
        };
        let json = serde_json::to_string(&restore).unwrap();
        assert_eq!(serde_json::from_str::<Edit>(&json).unwrap(), restore);
        let json = serde_json::to_value(Edit::RemoveSite { site: SiteId(2) }).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"command": "remove_site", "site": 2})
        );
        let values: CellValues = serde_json::from_str(r#"{"gamma": 120}"#).unwrap();
        assert_eq!(values, CellValues::default().with(CellParam::Gamma, 120.0));
    }
}
