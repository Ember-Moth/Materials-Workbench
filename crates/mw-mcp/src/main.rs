//! `mw-mcp`: the MCP server on its own, for development and the protocol
//! tests. Users start the same server as `materials-workbench mcp`, which
//! also lists the projects the application opened recently; this one reads
//! no configuration and lists none (ai-integration design §5.1).

use std::process::ExitCode;

fn main() -> ExitCode {
    mw_mcp::main("mw-mcp", std::env::args().skip(1), Box::new(Vec::new))
}
