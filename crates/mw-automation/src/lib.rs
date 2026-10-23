//! The capability registry (`docs/design/ai-integration.md` §4): every
//! operation the interface, scripts, the command line, MCP clients and the
//! built-in assistant can call, with an English description for models,
//! JSON Schemas for its input and output, and what it can change.
//!
//! Capabilities run on documents a [`Host`] provides: the running
//! application, or [`Documents`] held in memory. They call the same domain
//! commands as the interface (`mw_crystal::Edit`), name things by stable
//! ids ([`StructureEntryId`], `SiteId`, `AtomRef`), and fail with
//! recoverable errors that say what to do instead.
//!
//! The schemas come from the Rust types ([`schema`]) and stay within the
//! strict subset both model vendors accept (design §6.2.6).

mod files;
mod history;
mod registry;
pub mod schema;
mod structure;

pub use files::{MAX_EXPORT_BYTES, diagnostic_message};
pub use registry::{
    Availability, CallError, CallResult, Capability, Documents, Effect, Host, Output, Registry,
    Spec, StructureEntryId, StructureInfo,
};
