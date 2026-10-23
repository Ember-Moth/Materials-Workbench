//! Which files clients may open (ai-integration design §7.2): projects and
//! CIF files inside the directories the server was started with. MCP roots
//! are deprecated as of protocol 2026-07-28, which suggests server
//! configuration instead, so the directories come from the command line.

use std::path::{Component, Path, PathBuf};

use mw_automation::CallError;

/// Largest file the server reads. COD's largest CIF files are a few MB.
pub const MAX_FILE_BYTES: u64 = 64 << 20;

/// The directories clients may open files from.
#[derive(Clone, Debug, Default)]
pub struct Access {
    /// Canonical paths.
    allowed: Vec<PathBuf>,
}

impl Access {
    /// Allows the given directories. Each must exist; relative ones are
    /// taken from the working directory.
    pub fn new(directories: &[PathBuf]) -> std::io::Result<Self> {
        let allowed = directories
            .iter()
            .map(|dir| {
                let dir = dir.canonicalize()?;
                if dir.is_dir() {
                    Ok(dir)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotADirectory,
                        format!("{} is not a directory", dir.display()),
                    ))
                }
            })
            .collect::<std::io::Result<_>>()?;
        Ok(Self { allowed })
    }

    /// Allows the working directory, unless it is the file system root,
    /// where clients often start servers: that would allow every file.
    pub fn working_directory() -> std::io::Result<Self> {
        let dir = std::env::current_dir()?.canonicalize()?;
        let allowed = if dir.parent().is_none() {
            Vec::new()
        } else {
            vec![dir]
        };
        Ok(Self { allowed })
    }

    pub fn allowed(&self) -> &[PathBuf] {
        &self.allowed
    }

    fn contains(&self, path: &Path) -> bool {
        self.allowed.iter().any(|dir| path.starts_with(dir))
    }

    /// Whether clients may open `path`, a project the application
    /// remembers. One that is gone counts by its name.
    pub fn permits(&self, path: &Path) -> bool {
        match path.canonicalize() {
            Ok(resolved) => self.contains(&resolved),
            Err(_) => std::path::absolute(path).is_ok_and(|p| self.contains(&as_far_as_exists(&p))),
        }
    }

    fn refusal(&self, path: &str) -> CallError {
        let message = if self.allowed.is_empty() {
            format!(
                "{path} cannot be opened: the server was started without directories to open \
                 files from; the user can add them with --allow"
            )
        } else {
            format!(
                "{path} is outside the directories files can be opened from: {}",
                self.allowed
                    .iter()
                    .map(|dir| dir.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        CallError::new("not_allowed", message)
    }

    /// The canonical form of a path a client gave, if clients may open it.
    ///
    /// Paths are resolved, symbolic links included, before they are
    /// checked, so a link cannot lead outside the allowed directories.
    fn resolve(&self, path: &str) -> Result<PathBuf, CallError> {
        let given = Path::new(path);
        let absolute = std::path::absolute(given)
            .map_err(|e| CallError::new("invalid_arguments", format!("{path}: {e}")))?;
        let resolved = match absolute.canonicalize() {
            Ok(resolved) => resolved,
            // Only say a file is missing where the client may look.
            Err(_) if !self.contains(&as_far_as_exists(&absolute)) => {
                return Err(self.refusal(path));
            }
            Err(e) => return Err(CallError::new("not_found", format!("{path}: {e}"))),
        };
        if !self.contains(&resolved) {
            return Err(self.refusal(path));
        }
        Ok(resolved)
    }

    /// Checks the path of a project directory a client gave; returns it
    /// canonical. Its files are read through the project's own reader.
    pub fn project(&self, path: &str) -> Result<PathBuf, CallError> {
        let resolved = self.resolve(path)?;
        if !resolved.is_dir() {
            return Err(CallError::new(
                "not_a_project",
                format!("{path} is not a project: projects are directories named name.mwproj"),
            ));
        }
        Ok(resolved)
    }

    /// Checks a path a client gave and reads the file.
    pub fn read(&self, path: &str) -> Result<(PathBuf, Vec<u8>), CallError> {
        let resolved = self.resolve(path)?;
        let is_cif = resolved
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cif"));
        if !is_cif {
            return Err(CallError::new(
                "unsupported_format",
                format!("{path} is not a .cif file; only CIF files can be opened so far"),
            ));
        }
        let metadata = std::fs::metadata(&resolved)
            .map_err(|e| CallError::new("io_error", format!("{path}: {e}")))?;
        if !metadata.is_file() {
            return Err(CallError::new("not_found", format!("{path} is not a file")));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(CallError::new(
                "too_large",
                format!(
                    "{path} is {} bytes, more than the {MAX_FILE_BYTES} the server reads",
                    metadata.len()
                ),
            ));
        }
        let bytes = std::fs::read(&resolved)
            .map_err(|e| CallError::new("io_error", format!("{path}: {e}")))?;
        Ok((resolved, bytes))
    }
}

/// A path that does not exist, resolved as far as it does: links in the
/// part that exists (`/tmp` is `/private/tmp` on macOS), and `.` and `..`
/// by name.
fn as_far_as_exists(path: &Path) -> PathBuf {
    let path = lexical(path);
    let mut existing = path.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(mut resolved) = existing.canonicalize() {
            resolved.extend(rest.iter().rev());
            return resolved;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return path,
        }
    }
}

/// The path with `.` and `..` resolved by name, without following links.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    #[test]
    fn reads_cif_files_inside_allowed_directories() {
        let access = Access::new(&[fixtures()]).unwrap();
        let path = fixtures().join("rutile.cif");
        let (resolved, bytes) = access.read(path.to_str().unwrap()).unwrap();
        assert!(resolved.ends_with("rutile.cif"));
        assert!(!bytes.is_empty());
    }

    #[test]
    fn refuses_other_files() {
        let access = Access::new(&[fixtures()]).unwrap();
        let code = |path: PathBuf| access.read(path.to_str().unwrap()).unwrap_err().code;
        // Outside, even by way of `..`.
        let manifest = fixtures().join("../../Cargo.toml");
        assert_eq!(code(manifest), "not_allowed");
        let outside = fixtures().join("../../src/missing.cif");
        assert_eq!(code(outside), "not_allowed");
        assert_eq!(code(fixtures().join("missing.cif")), "not_found");
        assert_eq!(code(fixtures().join("README.md")), "unsupported_format");
        assert_eq!(
            Access::default().read("rutile.cif").unwrap_err().code,
            "not_allowed"
        );
    }

    #[test]
    fn projects_are_directories_inside_allowed_ones() {
        let temp = tempfile::tempdir().unwrap();
        let allowed = temp.path().join("allowed");
        let project = allowed.join("p.mwproj");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(allowed.join("x.cif"), "").unwrap();
        let access = Access::new(std::slice::from_ref(&allowed)).unwrap();
        let resolved = access.project(project.to_str().unwrap()).unwrap();
        assert_eq!(resolved, project.canonicalize().unwrap());
        let code = |path: PathBuf| access.project(path.to_str().unwrap()).unwrap_err().code;
        assert_eq!(code(allowed.join("x.cif")), "not_a_project");
        assert_eq!(code(allowed.join("q.mwproj")), "not_found");
        assert_eq!(code(temp.path().to_path_buf()), "not_allowed");

        assert!(access.permits(&project));
        // Gone, but where clients may look.
        assert!(access.permits(&allowed.join("gone.mwproj")));
        assert!(!access.permits(&temp.path().join("other.mwproj")));
        assert!(!access.permits(&allowed.join("../other.mwproj")));
    }
}
