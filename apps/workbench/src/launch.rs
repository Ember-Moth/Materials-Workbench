//! How the application is started (`docs/design/release.md` §2.2, §3.3):
//! its identity towards the operating systems, and what its arguments and
//! the system's requests to open files ask for.

use std::ffi::OsString;
use std::path::PathBuf;

/// The name the operating systems show.
pub const APP_NAME: &str = "Materials Workbench";

/// The application's identifier: the macOS bundle identifier
/// (`packaging/macos/Info.plist`) and the Windows AppUserModelID, which the
/// installer's shortcuts carry too (`packaging/windows/`). Never changes
/// once released: the systems tell an upgrade from a second application by
/// it.
pub const APP_ID: &str = "io.github.ember-moth.MaterialsWorkbench";

/// The Linux application ID: the window's Wayland app_id and X11 class,
/// which desktops match with the `.desktop` file of the same name
/// (`packaging/linux/`). Freedesktop IDs take `_` where [`APP_ID`] has `-`.
pub const LINUX_APP_ID: &str = "io.github.ember_moth.MaterialsWorkbench";

/// The help text.
pub const USAGE: &str = "\
Usage: materials-workbench [FILE | PROJECT]...
       materials-workbench mcp [--allow DIR]...

Opens the window, and in it the project folders (.mwproj) and imports the
structure files (CIF, POSCAR) given, after the projects open when the
application last quit.

Commands:
  mcp            Serve an MCP client on stdio, without a window.
                 `materials-workbench mcp --help` says more.

Options:
  -h, --help     Print this help.
  -V, --version  Print the version.
";

/// What the command line asks for.
#[derive(Debug, PartialEq)]
pub enum Launch {
    /// `materials-workbench mcp …`: the arguments after `mcp`.
    Mcp(Vec<String>),
    Help,
    Version,
    /// The window, and in it these files and project folders, absolute.
    Open(Vec<PathBuf>),
}

/// Reads the arguments after the program's name. Relative paths are taken
/// from the working directory. The error says what is wrong.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Launch, String> {
    let mut args = args.into_iter().peekable();
    if args
        .peek()
        .is_some_and(|first| first == crate::mcp::COMMAND)
    {
        args.next();
        return args
            .map(|arg| {
                arg.into_string()
                    .map_err(|arg| format!("not valid Unicode: {}", arg.to_string_lossy()))
            })
            .collect::<Result<_, _>>()
            .map(Launch::Mcp);
    }
    let mut paths = Vec::new();
    let mut options_end = false;
    for arg in args {
        let text = arg.to_string_lossy();
        if !options_end && text.starts_with('-') {
            match text.as_ref() {
                "-h" | "--help" => return Ok(Launch::Help),
                "-V" | "--version" => return Ok(Launch::Version),
                "--" => options_end = true,
                // The process serial number older macOS versions pass to
                // applications started from the Finder.
                _ if text.starts_with("-psn_") => {}
                _ => return Err(format!("unknown option {text}")),
            }
            continue;
        }
        let path = std::path::absolute(&arg).map_err(|e| format!("{text}: {e}"))?;
        paths.push(normalized(path));
    }
    Ok(Launch::Open(paths))
}

/// The files and folders among URLs the system asks the application to open
/// (macOS sends `file:` URLs); other URLs are left out.
pub fn paths_of_urls(urls: &[String]) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|text| {
            let path = url::Url::parse(text)
                .ok()
                .filter(|url| url.scheme() == "file")
                .and_then(|url| url.to_file_path().ok());
            if path.is_none() {
                log::warn!("not opening {text}: not a file");
            }
            path.map(normalized)
        })
        .collect()
}

/// Without `.` components and a trailing separator: a folder the system
/// names `…/tio2.mwproj/` is the project the recent list knows as
/// `…/tio2.mwproj`.
fn normalized(path: PathBuf) -> PathBuf {
    path.components().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Result<Launch, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn arguments_are_files_to_open() {
        let cwd = std::env::current_dir().unwrap();
        let Ok(Launch::Open(paths)) = parse_strs(&["rutile.cif", "/tmp/./tio2.mwproj/"]) else {
            panic!("paths to open");
        };
        assert_eq!(paths[0], cwd.join("rutile.cif"));
        #[cfg(unix)]
        assert_eq!(paths[1], PathBuf::from("/tmp/tio2.mwproj"));
        assert_eq!(parse_strs(&[]), Ok(Launch::Open(Vec::new())));
    }

    #[test]
    fn options_and_the_mcp_command() {
        assert_eq!(parse_strs(&["--version"]), Ok(Launch::Version));
        assert_eq!(parse_strs(&["a.cif", "-h"]), Ok(Launch::Help));
        assert_eq!(
            parse_strs(&["mcp", "--allow", "dir"]),
            Ok(Launch::Mcp(vec!["--allow".into(), "dir".into()]))
        );
        // `mcp` only as the first argument; elsewhere it is a file.
        assert!(matches!(parse_strs(&["a.cif", "mcp"]), Ok(Launch::Open(p)) if p.len() == 2));
        assert_eq!(
            parse_strs(&["--frobnicate"]),
            Err("unknown option --frobnicate".into())
        );
        assert_eq!(parse_strs(&["-psn_0_12345"]), Ok(Launch::Open(Vec::new())));
        let Ok(Launch::Open(paths)) = parse_strs(&["--", "-odd.cif"]) else {
            panic!("a path after --");
        };
        assert!(paths[0].ends_with("-odd.cif"));
    }

    #[cfg(unix)]
    #[test]
    fn file_urls_are_paths() {
        let urls = [
            "file:///Users/me/My%20Projects/tio2.mwproj/".to_string(),
            "file:///tmp/%E9%87%91%E7%BA%A2%E7%9F%B3.cif".to_string(),
            "https://example.com/rutile.cif".to_string(),
            "not a url".to_string(),
        ];
        assert_eq!(
            paths_of_urls(&urls),
            [
                PathBuf::from("/Users/me/My Projects/tio2.mwproj"),
                PathBuf::from("/tmp/金红石.cif"),
            ]
        );
    }
}
