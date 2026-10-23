//! Where the running application listens for `materials-workbench mcp`
//! (ai-integration design §5.1): a Unix domain socket in the user's
//! runtime directory on macOS and Linux, a named pipe on Windows, and next
//! to it a file with the token that proves a client is the same user.
//!
//! The directory is the user's own (mode 0700 on Unix), the socket and the
//! token file are 0600, and both ends check that the other process belongs
//! to the same user. The token is new at every start.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The application's name in the runtime directory.
const NAME: &str = "materials-workbench";
/// Version of the handshake ([`Hello`]).
pub(crate) const HANDSHAKE: u32 = 1;
/// Longest handshake line either end reads: the token and a few paths.
pub(crate) const MAX_HANDSHAKE: u64 = 64 << 10;

/// The current user's directory for the application's files that live as
/// long as it runs, `None` where the system has none: on macOS the user's
/// temporary directory, on Linux `$XDG_RUNTIME_DIR` (or systemd's
/// `/run/user/<uid>`, since some clients start servers without the
/// variable), on Windows the local application data directory. Found the
/// same way however the process was started, so that the application and
/// the server an MCP client starts agree on it.
pub fn user_runtime_dir() -> Option<PathBuf> {
    platform::runtime_dir().map(|dir| dir.join(NAME))
}

/// Where the application listens: a directory of the user's own, which
/// holds the socket and the token file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    dir: PathBuf,
}

impl Endpoint {
    /// The endpoint in `dir`, which is made when the application starts
    /// listening.
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file with the token, readable by the user alone.
    pub fn token_file(&self) -> PathBuf {
        self.dir.join("mcp.token")
    }

    /// The Unix domain socket.
    #[cfg(unix)]
    pub fn socket(&self) -> PathBuf {
        self.dir.join("mcp.sock")
    }

    /// The named pipe: one per directory, as the socket is on Unix.
    #[cfg(windows)]
    pub fn pipe(&self) -> String {
        let digest = mw_project_file::Sha256::of(self.dir.as_os_str().as_encoded_bytes());
        format!(r"\\.\pipe\{NAME}-{}", &digest.as_str()[..16])
    }
}

/// A random token, 256 bits from the system's generator.
pub(crate) fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Compares tokens in time that does not depend on where they differ.
pub(crate) fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// Writes the token through a temporary file, readable by the user alone
/// from the moment it exists.
pub(crate) fn write_token(path: &Path, token: &str) -> io::Result<()> {
    use std::io::Write as _;
    let temporary = path.with_extension("token.tmp");
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(token.as_bytes())?;
    drop(file);
    fs::rename(&temporary, path)
}

/// The token in the file; `None` when there is none, so the application is
/// not listening.
pub(crate) fn read_token(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(token) => Ok(Some(token.trim().to_owned())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// What `materials-workbench mcp` says first: the token, and what its
/// command line allows, which the application applies to its calls.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Hello {
    #[serde(rename = "materials-workbench-mcp")]
    pub handshake: u32,
    pub token: String,
    /// The directories the client may open projects from, canonical.
    pub allow: Vec<PathBuf>,
    /// Where relative paths in calls are taken from: the working directory
    /// of the process the client started.
    pub cwd: Option<PathBuf>,
}

/// The application's answer to [`Hello`]; after it, the connection carries
/// MCP's JSON-RPC as stdio does.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Welcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[cfg(unix)]
pub(crate) use platform::{check_private_dir, current_uid, make_private_dir};

#[cfg(unix)]
mod platform {
    use std::fs;
    use std::io;
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};

    pub fn current_uid() -> u32 {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() }
    }

    #[cfg(target_os = "macos")]
    pub fn runtime_dir() -> Option<PathBuf> {
        use std::os::unix::ffi::OsStringExt as _;
        // The directory `$TMPDIR` names, asked of the system: clients may
        // start servers without the variable.
        let mut buffer = vec![0u8; 1024];
        // SAFETY: the buffer is valid for writes of its length, which
        // confstr does not exceed.
        let length = unsafe {
            libc::confstr(
                libc::_CS_DARWIN_USER_TEMP_DIR,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
            )
        };
        if length == 0 || length > buffer.len() {
            return None;
        }
        // The length counts the terminating NUL.
        buffer.truncate(length - 1);
        Some(PathBuf::from(std::ffi::OsString::from_vec(buffer)))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn runtime_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| {
                let dir = PathBuf::from(format!("/run/user/{}", current_uid()));
                dir.is_dir().then_some(dir)
            })
    }

    fn not_private(dir: &Path, why: &str) -> io::Error {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is not private to this user: {why}", dir.display()),
        )
    }

    /// Checks that the directory belongs to this user and no one else can
    /// enter it.
    pub fn check_private_dir(dir: &Path) -> io::Result<()> {
        let metadata = fs::symlink_metadata(dir)?;
        if !metadata.is_dir() {
            return Err(not_private(dir, "not a directory"));
        }
        if metadata.uid() != current_uid() {
            return Err(not_private(dir, "it belongs to another user"));
        }
        if metadata.mode() & 0o077 != 0 {
            return Err(not_private(dir, "others have access"));
        }
        Ok(())
    }

    /// Makes the directory, mode 0700, or takes away others' access to it
    /// when it is this user's already.
    pub fn make_private_dir(dir: &Path) -> io::Result<()> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let metadata = fs::symlink_metadata(dir)?;
        if metadata.is_dir() && metadata.uid() == current_uid() && metadata.mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        check_private_dir(dir)
    }
}

#[cfg(windows)]
mod platform {
    use std::path::PathBuf;

    /// The local application data directory, private to the user by its
    /// inherited access rights.
    pub fn runtime_dir() -> Option<PathBuf> {
        dirs::data_local_dir()
    }
}

#[cfg(not(any(unix, windows)))]
mod platform {
    pub fn runtime_dir() -> Option<std::path::PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_and_compared_whole() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(same_token(&a, &a.clone()));
        assert!(!same_token(&a, &b));
        assert!(!same_token(&a, &a[..63]));
        assert!(!same_token(&a, ""));
    }

    #[test]
    fn the_token_file_is_private() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mcp.token");
        assert_eq!(read_token(&path).unwrap(), None);
        write_token(&path, "first").unwrap();
        write_token(&path, "second").unwrap();
        assert_eq!(read_token(&path).unwrap().as_deref(), Some("second"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_directory_is_made_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("a/run");
        make_private_dir(&dir).unwrap();
        let mode = |dir: &Path| fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        // An existing directory others could enter is closed to them.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(check_private_dir(&dir).is_err());
        make_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        check_private_dir(&dir).unwrap();
    }

    #[test]
    fn the_runtime_dir_is_found() {
        #[cfg(any(target_os = "macos", windows))]
        assert!(user_runtime_dir().is_some_and(|dir| dir.is_absolute()));
        #[cfg(target_os = "macos")]
        assert!(
            user_runtime_dir()
                .unwrap()
                .ends_with("T/materials-workbench")
        );
    }
}
