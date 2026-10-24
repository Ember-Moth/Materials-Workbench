//! Who writes a project (design §15): the process holding the operating
//! system's lock on the project's `.lock` file. The lock goes with the file
//! handle, so it is released when the process ends, however it ends.

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Timestamp;

/// The lock file's name inside a project directory.
pub const LOCK: &str = ".lock";

/// What the lock file says about its holder; only for messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockHolder {
    /// The program, e.g. `Materials Workbench` or `materials-workbench mcp`.
    pub holder: String,
    pub pid: u32,
    pub since: Timestamp,
}

impl fmt::Display for LockHolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (process {}, since {})",
            self.holder, self.pid, self.since
        )
    }
}

/// The right to write a project, held until dropped.
#[derive(Debug)]
pub struct ProjectLock {
    _file: File,
    path: PathBuf,
}

/// Why a project could not be locked.
#[derive(Debug)]
pub enum LockError {
    /// Another process holds it; what it wrote about itself, if that could
    /// be read.
    Held(Option<LockHolder>),
    Io(io::Error),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Held(Some(holder)) => write!(f, "{holder} is writing the project"),
            Self::Held(None) => f.write_str("another program is writing the project"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for LockError {}

impl ProjectLock {
    /// Locks the project in `dir` for this process, `holder` naming the
    /// program. Fails at once if another process holds the lock.
    pub fn acquire(dir: &Path, holder: &str) -> Result<Self, LockError> {
        let path = dir.join(LOCK);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(LockError::Io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(LockError::Held(read_holder(&path))),
            Err(TryLockError::Error(error)) => return Err(LockError::Io(error)),
        }
        let me = LockHolder {
            holder: holder.into(),
            pid: std::process::id(),
            since: Timestamp::now(),
        };
        // Only for messages: a failure to say who holds the lock is no
        // reason to give it up.
        let text = serde_json::to_vec(&me).expect("holders serialize");
        let _ = file
            .set_len(0)
            .and_then(|()| file.rewind())
            .and_then(|()| file.write_all(&text))
            .and_then(|()| file.flush());
        Ok(Self { _file: file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Who holds the lock, as its file says. Windows keeps a locked file from
/// being read by others, so there it is unknown.
fn read_holder(path: &Path) -> Option<LockHolder> {
    let mut text = String::new();
    File::open(path).ok()?.read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time() {
        let temp = tempfile::tempdir().unwrap();
        let lock = ProjectLock::acquire(temp.path(), "materials-workbench mcp").unwrap();
        assert_eq!(lock.path(), temp.path().join(LOCK));
        // Locks on separate handles exclude each other within one process
        // too, as between processes.
        match ProjectLock::acquire(temp.path(), "Materials Workbench") {
            Err(LockError::Held(holder)) => {
                if cfg!(unix) {
                    let holder = holder.unwrap();
                    assert_eq!(holder.holder, "materials-workbench mcp");
                    assert_eq!(holder.pid, std::process::id());
                }
            }
            other => panic!("{other:?}"),
        }
        drop(lock);
        let again = ProjectLock::acquire(temp.path(), "Materials Workbench").unwrap();
        // The file stays; only the lock comes and goes.
        drop(again);
        assert!(temp.path().join(LOCK).exists());
        ProjectLock::acquire(temp.path(), "x").unwrap();
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.mwproj");
        assert!(matches!(
            ProjectLock::acquire(&missing, "x"),
            Err(LockError::Io(_))
        ));
    }
}
