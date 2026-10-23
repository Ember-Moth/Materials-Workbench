//! Project files, independent of any UI (`docs/design/project-format.md`).
//!
//! A project is a directory named `*.mwproj`:
//!
//! ```text
//! name.mwproj/
//! ├── project.json          the manifest: format version, project id and
//! │                         name, entries with their provenance
//! ├── structures/<id>.json  one structure document per entry
//! ├── sources/<sha256>.cif  byte-exact copies of imported files
//! └── view.json             view state; deleting it loses no data
//! ```
//!
//! [`read_manifest`] opens a project (migrating older formats in memory),
//! the [`ProjectDir`] it returns loads the parts on demand, and [`save`]
//! writes it back: every file atomically, the manifest last, then files
//! nothing refers to any more are removed. View state is read and written
//! apart from the data ([`ProjectDir::read_view`], [`write_view`]), since
//! changing the view does not modify the project.

mod format;
mod manifest;
pub mod samples;
mod store;
mod structure_file;
mod time;
mod view;

pub use format::{Sha256, structure_sha256, to_json};
pub use manifest::{
    Created, Derived, Entry, EntryId, EntryKind, Import, Manifest, Operation, ProjectId,
    Provenance, SourceFormat,
};
pub use store::{ProjectDir, ReadError, Save, SaveError, Source, read_manifest, save, write_view};
pub use structure_file::StructureFile;
pub use time::Timestamp;
pub use view::{Camera, DisplayModel, EntryView, PanelTab, ViewState};

/// The format version this code writes and the newest it reads (design
/// §4): 2 since lattices can have an explicit orientation.
pub const VERSION: u32 = 2;

/// The extension of project directories.
pub const EXTENSION: &str = "mwproj";

/// The manifest's file name inside a project directory.
pub const MANIFEST: &str = "project.json";
