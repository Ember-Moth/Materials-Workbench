//! An experimental, read-only MCP server for the workbench
//! (`docs/design/ai-integration.md` §5): the capability registry's
//! read-only capabilities as tools, and each open structure's CIF text as a
//! resource.
//!
//! It runs without a window. A client opens CIF files with `file_open`,
//! limited to the directories the server was started with ([`Access`]);
//! the structures stay in this process under stable ids. Every result that
//! holds text from a file is marked untrusted (design §7.4). Connecting to
//! the running application, project files and write tools come later
//! (design §5.1, §9).

mod access;
mod server;
mod session;

pub use access::{Access, MAX_FILE_BYTES};
pub use server::{OPEN_TOOL, Server, UNTRUSTED, tool_name};
pub use session::{PROJECT, parse_resource_uri, resource_uri};
