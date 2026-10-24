//! The capability registry (`docs/design/ai-integration.md` §4): every
//! operation the interface, scripts, the command line, MCP clients and the
//! built-in assistant can call, with an English description for models,
//! JSON Schemas for its input and output, and what it can change.
//!
//! Capabilities run on documents a [`Host`] provides: the running
//! application, the MCP server reading project files ([`Projects`]), or
//! [`Documents`] held in memory. They call the same domain
//! commands as the interface (`mw_crystal::Edit`), name things by stable
//! ids ([`StructureEntryId`], `SiteId`, `AtomRef`), and fail with
//! recoverable errors that say what to do instead.
//!
//! The schemas come from the Rust types ([`schema`]) and stay within the
//! strict subset both model vendors accept (design §6.2.6).
//!
//! AI clients call in a [`Context`]: what the user allowed them, and
//! whether the user confirmed an irreversible call. The registry enforces
//! it (design §7.2) and hands every client call's record to the host for
//! the audit log (design §7.3).

mod files;
mod history;
mod policy;
mod project;
mod registry;
pub mod schema;
mod structure;

pub use files::{MAX_EXPORT_BYTES, diagnostic_message};
pub use policy::{Caller, Client, Context, Permissions};
pub use project::{
    EntryInfo, ImportedFile, OpenedFile, Opening, ProjectInfo, Projects, RecentProject,
    RecentProjects, SkippedBlock, duplicate_project, parse_import, read_error, resource_uri,
    unknown_structure,
};
pub use registry::{
    Answer, Availability, CallError, CallResult, Capability, Documents, Effect, Host, Output,
    Registry, Spec, StructureEntryId, StructureInfo,
};
pub use structure::{MAX_SUPERCELL_ATOMS, supercell_name};
