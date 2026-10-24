//! Crystal structure model, independent of any UI.
//!
//! The types follow `docs/design/crystallography-model.md`. A [`Structure`]
//! is the asymmetric unit plus its [`Symmetry`] operation set; [`Derived`]
//! expands it into the atoms of the unit cell, finds their bonds, with each
//! atom of the crystal named by an [`AtomRef`], and analyzes its symmetry:
//! the space group type, cell constraints, and each site's site symmetry,
//! coordinate constraints and Wyckoff position ([`SymmetryAnalysis`]). The
//! space group tables and the symmetry search come from moyo. A
//! [`DisplayRange`] chooses the part of the crystal the view shows, a block
//! of cells or a sphere, and gives its atoms and bonds ([`Displayed`]); in
//! the polyhedra model also the coordination [`Polyhedron`]s of the centers
//! [`PolyhedronCenters`] chooses.
//!
//! Editing (design §7): an [`Edit`] is a serializable command on whole
//! sites with an exact inverse; [`target_sites`] refuses edits of single
//! atoms that would break the symmetry. Under symmetry, moved and added
//! sites keep to their special positions and snap onto ones nearby. A [`CrystalDocument`] keeps a
//! structure with its derived data, measurements and undo history. A
//! [`CellTransform`] describes the structure in another cell; the
//! [`CellPreset`]s give the primitive, conventional and Niggli cells and the
//! other tabulated settings. [`DetectSymmetry`] finds the symmetry a
//! structure's atoms have and previews symmetrizing by it, which
//! [`Edit::Symmetrize`] commits.

mod atom_ref;
mod bonds;
mod cell_transform;
mod constraints;
mod derived;
mod display;
mod document;
mod edit;
pub mod element;
mod expand;
mod formula;
mod id_map;
mod lattice;
mod linalg;
mod measured;
mod neighbors;
mod placement;
mod polyhedra;
pub mod sample;
mod site;
mod space_group;
mod structure;
mod symmetrize;
mod symmetry;
mod tables;
mod transform;

pub use atom_ref::{AtomRef, Measurement, angle, distance};
pub use bonds::{Bond, BondRules, bonds, covalent_radius};
pub use cell_transform::{
    CellPreset, CellTransform, MAX_VOLUME_FACTOR, SettingChoice, TransformError, find_setting,
    other_settings,
};
pub use constraints::{CELL_DEVIATION, cell_deviation};
pub use derived::{
    Axis, CellConstraints, CellParam, Constraint, CoordinateConstraint, CrystalSystem,
    DETECTION_LIMIT, Derived, SymmetryAnalysis,
};
pub use display::{
    DisplayAtom, DisplayRange, DisplayRangeError, Displayed, MAX_DISPLAY_ATOMS, SphereCenter,
};
pub use document::{Annotation, Checkpoint, Command, CrystalDocument, Outcome};
pub use edit::{
    Applied, CellValues, Edit, EditError, EditInfo, EditWarning, Guidance, OCCUPANCY_TOLERANCE,
    Target, expand_and_remove, target_sites,
};
pub use element::UNKNOWN_ELEMENT;
pub use expand::{Contact, Orbit, close_contacts, orbits};
pub use formula::Formula;
pub use id_map::{AtomFate, IdMap, PositionMap, Rebasing, Relocation, SiteFate};
pub use lattice::{CellParams, Lattice, Orientation, Rotation};
pub use measured::Measured;
pub use neighbors::Neighbor;
pub use polyhedra::{
    CenterElement, Coordination, DEFAULT_MAX_COORDINATION, MAX_LIGANDS, Polyhedron,
    PolyhedronCenters,
};
pub use site::{CoordinateForm, SiteSymmetry, SpecialPosition, special_position};
pub use space_group::{
    DetectedSymmetry, Setting, SpaceGroupType, Wyckoff, detect_symmetry, it_number_of,
};
pub use structure::{
    Adp, Disorder, EditMode, Occupant, Site, SiteId, SpaceGroup, Species, Structure,
    StructureError, Tolerances,
};
pub use symmetrize::{DetectSymmetry, Symmetrization, SymmetrizeError};
pub use symmetry::{Rational, SymOp, SymOpError, Symmetry, SymmetryError};
pub use transform::{SupercellError, make_supercell, right_handed};
