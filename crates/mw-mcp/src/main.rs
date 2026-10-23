//! `mw-mcp`: the experimental MCP server over stdio. MCP clients start it
//! and talk to it on stdin and stdout; logs go to stderr (`RUST_LOG`).
//!
//! It stands alone while it is a prototype; later `materials-workbench mcp`
//! starts the same server (ai-integration design §5.1).

use std::path::PathBuf;
use std::process::ExitCode;

use mw_mcp::{Access, Server};
use rmcp::ServiceExt;

const USAGE: &str = "\
Usage: mw-mcp [--allow DIR]...

Serves Materials Workbench to an MCP client over stdio: open CIF files,
query structures, export them as text. Experimental and read-only.

Options:
  --allow DIR    Let clients open CIF files inside DIR and its subdirectories.
                 Repeatable. Default: the working directory, unless it is /.
  -h, --help     Print this help.
  -V, --version  Print the version.
";

fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Vec<PathBuf>>, String> {
    let mut allowed = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--allow" => {
                let dir = args.next().ok_or("--allow needs a directory")?;
                allowed.push(PathBuf::from(dir));
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("mw-mcp {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Some(allowed))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .target(env_logger::Target::Stderr)
        .init();
    let allowed = match parse(std::env::args().skip(1)) {
        Ok(Some(allowed)) => allowed,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("mw-mcp: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let access = if allowed.is_empty() {
        Access::working_directory()
    } else {
        Access::new(&allowed)
    };
    let access = match access {
        Ok(access) => access,
        Err(e) => {
            eprintln!("mw-mcp: {e}");
            return ExitCode::from(2);
        }
    };
    if access.allowed().is_empty() {
        log::warn!("no directory to open files from; start the server with --allow DIR");
    }
    let service = match Server::new(access).serve(rmcp::transport::stdio()).await {
        Ok(service) => service,
        Err(e) => {
            log::error!("the client did not start a session: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = service.waiting().await {
        log::error!("the session ended with an error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
