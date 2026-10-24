//! `mw-mcp`: the MCP server on its own, for development and the protocol
//! tests. Users start the same server as `materials-workbench mcp`, which
//! also lists the projects the application opened recently and connects to
//! the running application; this one reads no configuration, lists none,
//! lets clients do what the default settings allow and always runs without
//! a window (ai-integration design §5.1).

use std::process::ExitCode;

fn main() -> ExitCode {
    mw_mcp::main(
        "mw-mcp",
        std::env::args().skip(1),
        Box::new(Vec::new),
        Box::new(|_| mw_automation::Permissions::default()),
        None,
    )
}
