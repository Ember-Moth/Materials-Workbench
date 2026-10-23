//! An experimental, read-only MCP server for the workbench
//! (`docs/design/ai-integration.md` §5): the capability registry's
//! read-only capabilities as tools, and each open structure's CIF text as a
//! resource.
//!
//! The protocol side, [`Server`], works on any `mw_automation::Host`.
//! Without a window the host is [`Headless`]: clients open projects from
//! their files and CIF files outside any project, limited to the
//! directories the server was started with ([`Access`]), and nothing is
//! written back. Every result that holds text from a file is marked
//! untrusted (design §7.4). Connecting to the running application and
//! write tools come later (design §5.1, §9).

mod access;
mod headless;
mod server;

pub use access::{Access, MAX_FILE_BYTES};
pub use headless::{Headless, RecentSource};
pub use server::{Server, UNTRUSTED, tool_name};
