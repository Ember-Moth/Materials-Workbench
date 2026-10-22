//! Crystal structure model, independent of any UI.
//!
//! The types follow `docs/design/crystallography-model.md`. Only the parts
//! the current UI needs exist so far; symmetry operations, editing commands
//! and file formats come later. Values that design says are *derived*
//! (Wyckoff positions, constraints, display atoms) are supplied by hand in
//! [`sample`] until the symmetry engine exists — see [`Derived`].

mod derived;
mod lattice;
mod measured;
pub mod sample;
mod structure;

pub use derived::{
    Axis, CellConstraints, CellParam, Constraint, CoordinateConstraint, CrystalSystem, Derived,
    DisplayAtom, SiteSymmetry,
};
pub use lattice::{CellParams, Lattice};
pub use measured::Measured;
pub use structure::{EditMode, Occupant, Site, SiteId, SpaceGroup, Species, Structure};
