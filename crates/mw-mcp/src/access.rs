//! Which files clients may open and write (ai-integration design §7.2):
//! projects and structure files inside the directories the server was
//! started with, and files inside the open projects' directories. MCP roots
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
    /// Where relative paths are taken from, when not from this process's
    /// working directory: the running application serves clients started
    /// elsewhere (design §5.1).
    base: Option<PathBuf>,
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
        Ok(Self {
            allowed,
            base: None,
        })
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
        Ok(Self {
            allowed,
            base: None,
        })
    }

    /// Takes relative paths from `dir`, an absolute path, instead of the
    /// working directory.
    pub fn with_base(mut self, dir: PathBuf) -> Self {
        self.base = Some(dir);
        self
    }

    pub fn allowed(&self) -> &[PathBuf] {
        &self.allowed
    }

    fn absolute(&self, path: &Path) -> std::io::Result<PathBuf> {
        match &self.base {
            Some(base) if path.is_relative() => std::path::absolute(base.join(path)),
            _ => std::path::absolute(path),
        }
    }

    fn contains(&self, path: &Path) -> bool {
        self.allowed.iter().any(|dir| path.starts_with(dir))
    }

    fn contains_in(&self, path: &Path, projects: &[PathBuf]) -> bool {
        self.contains(path) || projects.iter().any(|dir| path.starts_with(dir))
    }

    /// Whether clients may open `path`, a project the application
    /// remembers. One that is gone counts by its name.
    pub fn permits(&self, path: &Path) -> bool {
        match self.absolute(path).and_then(|p| p.canonicalize()) {
            Ok(resolved) => self.contains(&resolved),
            Err(_) => self
                .absolute(path)
                .is_ok_and(|p| self.contains(&as_far_as_exists(&p))),
        }
    }

    fn refusal(&self, path: &str) -> CallError {
        let message = if self.allowed.is_empty() {
            format!(
                "{path} is not allowed: the server was started without directories to open \
                 files from, so only open projects' directories are; the user can add \
                 directories with --allow"
            )
        } else {
            format!(
                "{path} is outside the directories files can be opened from and written to, \
                 apart from open projects' directories: {}",
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
        self.resolve_in(path, &[])
    }

    fn resolve_in(&self, path: &str, projects: &[PathBuf]) -> Result<PathBuf, CallError> {
        let absolute = self
            .absolute(Path::new(path))
            .map_err(|e| CallError::new("invalid_arguments", format!("{path}: {e}")))?;
        let resolved = match absolute.canonicalize() {
            Ok(resolved) => resolved,
            // Only say a file is missing where the client may look.
            Err(_) if !self.contains_in(&as_far_as_exists(&absolute), projects) => {
                return Err(self.refusal(path));
            }
            Err(e) => return Err(CallError::new("not_found", format!("{path}: {e}"))),
        };
        if !self.contains_in(&resolved, projects) {
            return Err(self.refusal(path));
        }
        Ok(resolved)
    }

    /// The canonical path of a file a client asked to write: inside the
    /// allowed directories or `projects`, the open projects' canonical
    /// directories. The file need not exist, its directory must. A link at
    /// the path is replaced, not followed, so it cannot lead outside.
    pub fn writable(&self, path: &str, projects: &[PathBuf]) -> Result<PathBuf, CallError> {
        let absolute = self
            .absolute(Path::new(path))
            .map(|p| lexical(&p))
            .map_err(|e| CallError::new("invalid_arguments", format!("{path}: {e}")))?;
        let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) else {
            return Err(CallError::new(
                "invalid_arguments",
                format!("{path} names no file"),
            ));
        };
        let dir = match parent.canonicalize() {
            Ok(dir) => dir,
            Err(_) if !self.contains_in(&as_far_as_exists(parent), projects) => {
                return Err(self.refusal(path));
            }
            Err(e) => {
                return Err(CallError::new(
                    "not_found",
                    format!("the directory of {path} does not exist: {e}"),
                ));
            }
        };
        let resolved = dir.join(name);
        if !self.contains_in(&resolved, projects) {
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
        self.read_in(path, &[])
    }

    /// Reads a file as [`Self::read`] does, also from `projects`, the open
    /// projects' canonical directories.
    pub fn read_in(
        &self,
        path: &str,
        projects: &[PathBuf],
    ) -> Result<(PathBuf, Vec<u8>), CallError> {
        let resolved = self.resolve_in(path, projects)?;
        if mw_project_file::SourceFormat::of_path(&resolved).is_none() {
            return Err(CallError::new(
                "unsupported_format",
                format!(
                    "{path} is neither CIF (.cif) nor POSCAR (.vasp, .poscar, POSCAR*, \
                     CONTCAR*)"
                ),
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

    #[test]
    fn files_are_written_inside_allowed_directories_and_projects() {
        let temp = tempfile::tempdir().unwrap();
        let allowed = temp.path().join("allowed");
        let project = temp.path().join("elsewhere/p.mwproj");
        std::fs::create_dir_all(&allowed).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let access = Access::new(std::slice::from_ref(&allowed)).unwrap();
        let projects = [project.canonicalize().unwrap()];
        let write = |path: PathBuf| access.writable(path.to_str().unwrap(), &projects);
        let target = write(allowed.join("POSCAR")).unwrap();
        assert_eq!(target, allowed.canonicalize().unwrap().join("POSCAR"));
        assert!(write(project.join("out.cif")).is_ok());
        let code = |path: PathBuf| write(path).unwrap_err().code;
        assert_eq!(code(temp.path().join("POSCAR")), "not_allowed");
        assert_eq!(code(allowed.join("../POSCAR")), "not_allowed");
        assert_eq!(code(allowed.join("missing/POSCAR")), "not_found");
        assert_eq!(code(temp.path().join("missing/POSCAR")), "not_allowed");
        // Without the project, its directory is out of bounds.
        assert_eq!(
            access
                .writable(project.join("out.cif").to_str().unwrap(), &[])
                .unwrap_err()
                .code,
            "not_allowed"
        );
        // Reading follows the same bounds.
        std::fs::write(project.join("x.cif"), "data_x").unwrap();
        let path = project.join("x.cif");
        assert!(access.read_in(path.to_str().unwrap(), &projects).is_ok());
        assert_eq!(
            access.read(path.to_str().unwrap()).unwrap_err().code,
            "not_allowed"
        );
    }

    #[test]
    fn relative_paths_from_another_directory() {
        let access = Access::new(&[fixtures()])
            .unwrap()
            .with_base(fixtures().join("../.."));
        let (resolved, _) = access.read("tests/fixtures/rutile.cif").unwrap();
        assert!(resolved.ends_with("rutile.cif"));
        assert_eq!(access.read("Cargo.toml").unwrap_err().code, "not_allowed");
    }
}
