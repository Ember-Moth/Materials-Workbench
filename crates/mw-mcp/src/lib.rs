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
//! untrusted (design §7.4). [`main`] runs it for `materials-workbench mcp`
//! and the standalone `mw-mcp`.
//!
//! The running application [`listen`]s at an [`Endpoint`] in the user's
//! runtime directory; `materials-workbench mcp` finds it there and relays
//! stdio to it. The application serves each connection with a [`Server`]
//! whose calls it runs on its main thread ([`dispatcher`], [`Calls`]).
//! Write tools come later (design §5.1, §9).

mod access;
mod cli;
mod dispatch;
mod endpoint;
mod headless;
mod listen;
mod relay;
mod server;

pub use access::{Access, MAX_FILE_BYTES};
pub use cli::main;
pub use dispatch::{Call, Calls, Dispatcher, dispatcher};
pub use endpoint::{Endpoint, user_runtime_dir};
pub use headless::{Headless, RecentSource};
pub use listen::{Listening, listen};
/// What [`RecentSource`] gives.
pub use mw_automation::RecentProject;
pub use server::{Server, UNTRUSTED, tool_name};
