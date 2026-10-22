//! Crystal structure model, independent of any UI.
//!
//! The types follow `docs/design/crystallography-model.md`. A [`Structure`]
//! is the asymmetric unit plus its [`Symmetry`] operation set; [`Derived`]
//! expands it into the atoms of the unit cell, finds their bonds, with each
//! atom of the crystal named by an [`AtomRef`], and analyzes its symmetry:
//! the space group type, cell constraints, and each site's site symmetry,
//! coordinate constraints and Wyckoff position ([`SymmetryAnalysis`]). The
//! space group tables and the symmetry search come from moyo. Editing
//! commands come later.

mod atom_ref;
mod bonds;
mod constraints;
mod derived;
pub mod element;
mod expand;
mod lattice;
mod linalg;
mod measured;
pub mod sample;
mod site;
mod space_group;
mod structure;
mod symmetry;
mod tables;

pub use atom_ref::{AtomRef, Measurement, angle, distance};
pub use bonds::{Bond, BondRules, covalent_radius, display_bonds};
pub use constraints::cell_deviation;
pub use derived::{
    Axis, CellConstraints, CellParam, Constraint, CoordinateConstraint, CrystalSystem,
    DETECTION_LIMIT, Derived, DisplayAtom, SymmetryAnalysis,
};
pub use element::UNKNOWN_ELEMENT;
pub use expand::{Contact, Orbit, close_contacts, display_atoms, orbits};
pub use lattice::{CellParams, Lattice};
pub use measured::Measured;
pub use site::{CoordinateForm, SiteSymmetry, SpecialPosition, special_position};
pub use space_group::{
    DetectedSymmetry, Setting, SpaceGroupType, Wyckoff, detect_symmetry, it_number_of,
};
pub use structure::{
    Adp, Disorder, EditMode, Occupant, Site, SiteId, SpaceGroup, Species, Structure, Tolerances,
};
pub use symmetry::{Rational, SymOp, SymOpError, Symmetry, SymmetryError};
