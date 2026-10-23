//! The server's command line, shared by `materials-workbench mcp` and the
//! standalone `mw-mcp` (ai-integration design §5.1): the arguments, logging
//! to stderr, and the server on stdio until the client disconnects.

use std::path::PathBuf;
use std::process::ExitCode;

use rmcp::ServiceExt;

use crate::{Access, Headless, RecentSource, Server};

fn usage(program: &str) -> String {
    format!(
        "\
Usage: {program} [--allow DIR]...

Serves Materials Workbench to an MCP client over stdio: open projects and
CIF files, query structures, export them as text. Experimental and
read-only: nothing is written to disk.

Options:
  --allow DIR    Let clients open projects and CIF files inside DIR and its
                 subdirectories. Repeatable. Default: the working directory,
                 unless it is /.
  -h, --help     Print this help.
  -V, --version  Print the version.
"
    )
}

/// The directories to allow, or `None` when the arguments asked for help
/// or the version, which are printed.
fn parse(
    program: &str,
    mut args: impl Iterator<Item = String>,
) -> Result<Option<Vec<PathBuf>>, String> {
    let mut allowed = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--allow" => {
                let dir = args.next().ok_or("--allow needs a directory")?;
                allowed.push(PathBuf::from(dir));
            }
            "-h" | "--help" => {
                print!("{}", usage(program));
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("{program} {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(Some(allowed))
}

/// Runs the server for a command line and returns the process's exit
/// code. `program` is how messages name the command; `args` are the
/// arguments after it; recent projects come from `recent`. Blocks until the
/// client disconnects. Logs go to stderr (`RUST_LOG`), since stdout carries
/// the protocol.
pub fn main(
    program: &str,
    args: impl IntoIterator<Item = String>,
    recent: RecentSource,
) -> ExitCode {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .target(env_logger::Target::Stderr)
        .try_init();
    let allowed = match parse(program, args.into_iter()) {
        Ok(Some(allowed)) => allowed,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{program}: {message}\n\n{}", usage(program));
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
            eprintln!("{program}: {e}");
            return ExitCode::from(2);
        }
    };
    if access.allowed().is_empty() {
        log::warn!(
            "no directory to open projects and files from; start the server with --allow DIR"
        );
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("{program}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let host = Headless::new(access).with_recent(recent);
    runtime.block_on(async move {
        let service = match Server::new(host).serve(rmcp::transport::stdio()).await {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(args: &[&str]) -> impl Iterator<Item = String> {
        args.iter()
            .map(|a| a.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn arguments() {
        assert_eq!(
            parse("mcp", args(&["--allow", "a", "--allow", "b"])),
            Ok(Some(vec![PathBuf::from("a"), PathBuf::from("b")]))
        );
        assert_eq!(parse("mcp", args(&[])), Ok(Some(Vec::new())));
        assert!(parse("mcp", args(&["--allow"])).is_err());
        assert!(parse("mcp", args(&["--write"])).is_err());
    }
}
