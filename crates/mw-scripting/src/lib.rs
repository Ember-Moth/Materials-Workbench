//! What the command line (`mw-cli`) and the Python module (`mw-python`)
//! share (`docs/design/scripting.md`): structure files read by the
//! interface's rules, and each structure held as a [`Document`] that works
//! through the capability registry (`mw-automation`), so that every entry
//! point gives the same results.
//!
//! [`read_file`] tells the format from the file name as the interface does,
//! reads the file with `mw-crystal-io` and names its structures as the
//! interface names them. A [`Document`] holds one structure in an in-memory
//! host (`mw_automation::Documents`): queries, the change of cell and
//! symmetrizing are the registry's capabilities; making a supercell and
//! writing file text call the same domain functions as the interface,
//! since the registry does not have them (or limits their size) yet.
//! [`FileReport::diagnostics_json`] gives what reading a file found, each
//! diagnostic as `structure.diagnostics` gives it.

mod diagnostics;
mod document;
mod format;
mod read;

pub use diagnostics::{diagnostic_json, diagnostic_line, failure_text, severity_name};
pub use document::{
    Document, Exported, MAX_SUPERCELL_ATOMS, NewCell, capabilities_json, effect_name, registry,
};
pub use format::Format;
pub use read::{FileReport, ReadError, read_bytes, read_file};
