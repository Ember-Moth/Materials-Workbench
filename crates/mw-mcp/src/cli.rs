//! The server's command line, shared by `materials-workbench mcp` and the
//! standalone `mw-mcp` (ai-integration design §5.1): the arguments, logging
//! to stderr, and until the client disconnects either the relay to the
//! running application or the server on stdio without a window.

use std::path::PathBuf;
use std::process::ExitCode;

use rmcp::ServiceExt;

use crate::{Access, Endpoint, Headless, RecentSource, Server, relay};

fn usage(program: &str) -> String {
    format!(
        "\
Usage: {program} [--allow DIR]...

Serves Materials Workbench to an MCP client over stdio: open projects and
CIF files, query structures, export them as text. Experimental and
read-only: nothing is written to disk. When the application runs and lets
MCP clients connect (Settings), the client works with the projects open in
it instead.

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
/// arguments after it; recent projects come from `recent`. When the
/// application listens at `application`, the client is connected to it
/// instead. Blocks until the client disconnects. Logs go to stderr
/// (`RUST_LOG`), since stdout carries the protocol.
pub fn main(
    program: &str,
    args: impl IntoIterator<Item = String>,
    recent: RecentSource,
    application: Option<Endpoint>,
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
    let code = runtime.block_on(async move {
        if let Some(endpoint) = application {
            match relay::connect(&endpoint, &access).await {
                Ok(Some(link)) => {
                    log::info!("connected to the running application");
                    let input = tokio::io::stdin();
                    let output = tokio::io::stdout();
                    return match relay::relay(link, input, output).await {
                        Ok(()) => ExitCode::SUCCESS,
                        Err(e) => {
                            log::error!("the connection to the application failed: {e}");
                            ExitCode::FAILURE
                        }
                    };
                }
                Ok(None) => {}
                Err(e) => log::warn!(
                    "could not connect to the running application, serving without it: {e}"
                ),
            }
        }
        serve(Headless::new(access).with_recent(recent)).await
    });
    // A read of stdin may still be waiting when the application closes the
    // connection; it cannot be cancelled, and nothing needs it.
    runtime.shutdown_background();
    code
}

/// Serves the client on stdio without a window.
async fn serve(host: Headless) -> ExitCode {
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
