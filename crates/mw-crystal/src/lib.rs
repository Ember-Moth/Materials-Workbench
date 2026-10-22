//! Crystal structure model, independent of any UI.
//!
//! The types follow `docs/design/crystallography-model.md`. A [`Structure`]
//! is the asymmetric unit plus its [`Symmetry`] operation set; [`Derived`]
//! expands it into the atoms of the unit cell and finds their bonds, with
//! each atom of the crystal named by an [`AtomRef`]. What the symmetry engine (M2)
//! will derive — Wyckoff positions, coordinate and cell constraints — is only
//! available for the built-in [`sample`]s, written by hand; see
//! [`SymmetryAnalysis`]. Editing commands come later.

mod atom_ref;
mod bonds;
mod derived;
pub mod element;
mod expand;
mod lattice;
mod measured;
pub mod sample;
mod structure;
mod symmetry;

pub use atom_ref::{AtomRef, Measurement, angle, distance};
pub use bonds::{Bond, BondRules, covalent_radius, display_bonds};
pub use derived::{
    Axis, CellConstraints, CellParam, Constraint, CoordinateConstraint, CrystalSystem, Derived,
    DisplayAtom, SiteSymmetry, SymmetryAnalysis,
};
pub use element::UNKNOWN_ELEMENT;
pub use expand::{Contact, Orbit, close_contacts, display_atoms, orbits};
pub use lattice::{CellParams, Lattice};
pub use measured::Measured;
pub use structure::{
    Adp, Disorder, EditMode, Occupant, Site, SiteId, SpaceGroup, Species, Structure, Tolerances,
};
pub use symmetry::{Rational, SymOp, SymOpError, Symmetry, SymmetryError};
