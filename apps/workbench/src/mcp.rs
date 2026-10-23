//! `materials-workbench mcp`: the MCP server without a window
//! (`docs/design/ai-integration.md` §5.1), started instead of the
//! interface. It lists the projects the application opened recently, read
//! from the configuration directory on every call, and writes nothing
//! there.

use std::process::ExitCode;

use mw_mcp::RecentProject;

use crate::config::{AppPaths, Session};

/// The first argument that starts the server instead of the interface.
pub const COMMAND: &str = "mcp";

/// Runs the server with the arguments after `mcp`; returns when the client
/// disconnects.
pub fn run(args: impl IntoIterator<Item = String>) -> ExitCode {
    let paths = AppPaths::from_environment();
    let recent = move || {
        Session::load(&paths)
            .map(|session| session.recent)
            .unwrap_or_default()
            .into_iter()
            .map(|recent| RecentProject::new(recent.id, recent.name, recent.path, recent.opened_at))
            .collect()
    };
    mw_mcp::main("materials-workbench mcp", args, Box::new(recent))
}
