//! Project files, independent of any UI (`docs/design/project-format.md`).
//!
//! A project is a directory named `*.mwproj`:
//!
//! ```text
//! name.mwproj/
//! ├── project.json          the manifest: format version, project id and
//! │                         name, entries with their provenance
//! ├── structures/<id>.json  one structure document per entry
//! ├── sources/<sha256>.cif  byte-exact copies of imported files (.vasp for
//! │                         POSCAR)
//! ├── view.json             view state; deleting it loses no data
//! ├── audit.jsonl           the audit log of AI operations
//! └── .lock                 locked by the process that writes the project
//! ```
//!
//! [`read_manifest`] opens a project (migrating older formats in memory),
//! the [`ProjectDir`] it returns loads the parts on demand, and [`save`]
//! writes it back: every file atomically, the manifest last, then files
//! nothing refers to any more are removed. View state is read and written
//! apart from the data ([`ProjectDir::read_view`], [`write_view`]), since
//! changing the view does not modify the project.
//!
//! Only one process writes a project at a time: it holds a [`ProjectLock`]
//! (design §15). The MCP server without a window backs up what it replaces
//! ([`SessionBackup`]), and AI operations are logged ([`append_audit`]).

mod audit;
mod backup;
mod format;
mod lock;
mod manifest;
pub mod samples;
mod store;
mod structure_file;
mod time;
mod view;

pub use audit::{
    AUDIT, AuditClient, AuditEffect, AuditRecord, AuditResult, append_audit, read_audit,
};
pub use backup::{KEEP_BACKUPS, SessionBackup};
pub use format::{Sha256, structure_sha256, to_json};
pub use lock::{LOCK, LockError, LockHolder, ProjectLock};
pub use manifest::{
    Created, Derived, Entry, EntryId, EntryKind, Import, Manifest, Operation, ProjectId,
    Provenance, SourceFormat,
};
pub use store::{ProjectDir, ReadError, Save, SaveError, Source, read_manifest, save, write_view};
pub use structure_file::StructureFile;
pub use time::Timestamp;
pub use view::{Camera, DisplayModel, DisplayRange, EntryView, PanelTab, SphereCenter, ViewState};

/// The format version this code writes and the newest it reads (design
/// §4): 2 since lattices can have an explicit orientation and structures
/// can be imported from POSCAR.
pub const VERSION: u32 = 2;

/// The extension of project directories.
pub const EXTENSION: &str = "mwproj";

/// The manifest's file name inside a project directory.
pub const MANIFEST: &str = "project.json";
