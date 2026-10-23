//! Reading and writing project directories (design §4, §9).

use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mw_crystal::StructureError;
use serde::Serialize;
use serde_json::Value;

use crate::{
    EntryId, Import, MANIFEST, Manifest, Sha256, SourceFormat, StructureFile, VERSION, ViewState,
    to_json,
};

/// Identifies project manifests.
const FORMAT: &str = "materials-workbench/project";
const WRITER: &str = concat!("Materials Workbench ", env!("CARGO_PKG_VERSION"));
const VIEW: &str = "view.json";
const VIEW_VERSION: u32 = 1;
const STRUCTURES: &str = "structures";
const SOURCES: &str = "sources";

/// The files of a format version.
#[derive(Clone, Copy, Debug)]
enum FileKind {
    Manifest,
    Structure,
}

/// Upgrades a file from format version `n + 1` to `n + 2`, in memory
/// (design §4).
type Upgrade = fn(FileKind, &mut Value);
const UPGRADES: [Upgrade; VERSION as usize - 1] = [v1_to_v2];

/// Version 2 adds an optional `orientation` to a structure's `cell`
/// (crystallography design §4.2) and the import format `poscar`. A version
/// 1 cell has no orientation, which reads as the standard one, and no entry
/// was imported from POSCAR, so nothing needs rewriting.
fn v1_to_v2(_kind: FileKind, _value: &mut Value) {}

fn migrate(kind: FileKind, mut value: Value, from: u32) -> Value {
    for upgrade in &UPGRADES[from as usize - 1..] {
        upgrade(kind, &mut value);
    }
    value
}

/// A project directory as it was opened: where it is and which format
/// version its files are in. Cheap to clone, for reading its parts in the
/// background.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectDir {
    path: PathBuf,
    version: u32,
}

impl ProjectDir {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the files are in an older format: read through migrations,
    /// and backed up on the first save (design §4).
    pub fn is_migrated(&self) -> bool {
        self.version < VERSION
    }

    fn structure_path(&self, id: EntryId) -> PathBuf {
        structure_path(&self.path, id)
    }

    /// The structure document of an entry.
    pub fn read_structure(&self, id: EntryId) -> Result<StructureFile, ReadError> {
        let path = self.structure_path(id);
        let value = read_json(&path)?;
        let value = migrate(FileKind::Structure, value, self.version);
        let file: StructureFile =
            serde_json::from_value(value).map_err(|e| ReadError::Corrupt {
                path: path.clone(),
                reason: e.to_string(),
            })?;
        file.structure
            .validate()
            .map_err(|error| ReadError::Invalid { path, error })?;
        Ok(file)
    }

    /// The bytes of an imported file's copy, checked against its digest.
    pub fn read_source(&self, import: &Import) -> Result<Vec<u8>, ReadError> {
        let path = self.path.join(import.copy_path());
        let bytes = fs::read(&path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => ReadError::SourceMissing(path.clone()),
            _ => ReadError::Io {
                path: path.clone(),
                error,
            },
        })?;
        if Sha256::of(&bytes) != import.sha256 {
            return Err(ReadError::SourceChanged(path));
        }
        Ok(bytes)
    }

    /// The view state, or `None` if there is none or it cannot be read:
    /// view state is dropped rather than reported (design §8).
    pub fn read_view(&self) -> Option<ViewState> {
        let bytes = fs::read(self.path.join(VIEW)).ok()?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        if value.get("version")?.as_u64()? != u64::from(VIEW_VERSION) {
            return None;
        }
        serde_json::from_value(value).ok()
    }
}

/// Opens the project in `path`: reads its manifest, migrating an older
/// format in memory. Structures are read on demand.
pub fn read_manifest(path: &Path) -> Result<(ProjectDir, Manifest), ReadError> {
    let manifest_path = path.join(MANIFEST);
    let value = read_json(&manifest_path).map_err(|error| match error {
        ReadError::Io { error, .. } if error.kind() == io::ErrorKind::NotFound => {
            ReadError::NotAProject(path.to_path_buf())
        }
        error => error,
    })?;
    if value.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(ReadError::NotAProject(path.to_path_buf()));
    }
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .filter(|&v| v >= 1)
        .ok_or_else(|| ReadError::Corrupt {
            path: manifest_path.clone(),
            reason: "no format version".into(),
        })?;
    if version > VERSION {
        return Err(ReadError::Newer { version });
    }
    let value = migrate(FileKind::Manifest, value, version);
    let manifest: Manifest = serde_json::from_value(value).map_err(|e| ReadError::Corrupt {
        path: manifest_path.clone(),
        reason: e.to_string(),
    })?;
    let mut ids = HashSet::new();
    if let Some(entry) = manifest.entries.iter().find(|e| !ids.insert(e.id)) {
        return Err(ReadError::Corrupt {
            path: manifest_path,
            reason: format!("entry {} is listed twice", entry.id),
        });
    }
    let dir = ProjectDir {
        path: path.to_path_buf(),
        version,
    };
    Ok((dir, manifest))
}

/// Writes the view state, which does not count as a change to the project.
pub fn write_view(path: &Path, view: &ViewState) -> io::Result<()> {
    #[derive(Serialize)]
    struct ViewFile<'a> {
        version: u32,
        #[serde(flatten)]
        view: &'a ViewState,
    }
    let bytes = to_json(&ViewFile {
        version: VIEW_VERSION,
        view,
    })
    .map_err(io::Error::other)?;
    write_atomic(&path.join(VIEW), &bytes)
}

/// The copy of an imported file to add to a project.
#[derive(Clone, Debug)]
pub struct Source {
    pub sha256: Sha256,
    pub format: SourceFormat,
    pub bytes: Arc<[u8]>,
}

/// What to write when saving a project.
#[derive(Clone, Debug)]
pub struct Save {
    pub manifest: Manifest,
    /// Entries whose content changed since they were read, or that were
    /// never written.
    pub structures: Vec<(EntryId, StructureFile)>,
    /// Copies of imported files that may not be in the project yet.
    pub sources: Vec<Source>,
    /// The project as it was opened, if it was: entries and copies not
    /// given above come from it.
    pub from: Option<ProjectDir>,
    pub view: Option<ViewState>,
}

/// Writes a project to `path` (design §9): new copies of imported files,
/// changed structures, then the manifest, which makes the save count; then
/// removes files nothing refers to any more, and writes the view state.
/// Every file is replaced atomically. Saving to another directory than
/// `save.from` copies everything over.
pub fn save(path: &Path, save: &Save) -> Result<ProjectDir, SaveError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |error| SaveError::Io { path, error }
    };
    let structures = path.join(STRUCTURES);
    let sources = path.join(SOURCES);
    for dir in [path, &structures, &sources] {
        fs::create_dir_all(dir).map_err(io(dir))?;
    }
    let from = save.from.as_ref();
    let same_dir = from.is_some_and(|from| same_file(&from.path, path));
    if let Some(from) = from.filter(|from| same_dir && from.is_migrated()) {
        back_up(from)?;
    }

    // Copies of imported files are named by their content: one that is
    // there already is the same file.
    for import in save
        .manifest
        .entries
        .iter()
        .filter_map(|e| e.provenance.import())
    {
        let target = path.join(import.copy_path());
        if target.exists() {
            continue;
        }
        if let Some(source) = save.sources.iter().find(|s| s.sha256 == import.sha256) {
            write_atomic(&target, &source.bytes).map_err(io(&target))?;
        } else if let Some(from) = from {
            // A copy missing from the old project stays missing.
            let old = from.path.join(import.copy_path());
            if old.exists() {
                fs::copy(&old, &target).map_err(io(&target))?;
            }
        }
    }

    for entry in &save.manifest.entries {
        let target = structure_path(path, entry.id);
        if let Some((_, file)) = save.structures.iter().find(|(id, _)| *id == entry.id) {
            let bytes = to_json(file).expect("structure files serialize");
            write_atomic(&target, &bytes).map_err(io(&target))?;
        } else if let Some(from) = from {
            if from.is_migrated() {
                let file = from.read_structure(entry.id).map_err(SaveError::Read)?;
                let bytes = to_json(&file).expect("structure files serialize");
                write_atomic(&target, &bytes).map_err(io(&target))?;
            } else if !same_dir {
                let old = from.structure_path(entry.id);
                fs::copy(&old, &target).map_err(io(&old))?;
            }
        } else {
            return Err(SaveError::Missing(entry.id));
        }
    }

    #[derive(Serialize)]
    struct ManifestFile<'a> {
        format: &'static str,
        version: u32,
        writer: &'static str,
        #[serde(flatten)]
        manifest: &'a Manifest,
    }
    let manifest = path.join(MANIFEST);
    let bytes = to_json(&ManifestFile {
        format: FORMAT,
        version: VERSION,
        writer: WRITER,
        manifest: &save.manifest,
    })
    .expect("manifests serialize");
    write_atomic(&manifest, &bytes).map_err(io(&manifest))?;

    remove_unreferenced(path, &save.manifest);
    if let Some(view) = &save.view {
        write_view(path, view).map_err(io(&path.join(VIEW)))?;
    }
    Ok(ProjectDir {
        path: path.to_path_buf(),
        version: VERSION,
    })
}

fn structure_path(dir: &Path, id: EntryId) -> PathBuf {
    dir.join(STRUCTURES).join(format!("{id}.json"))
}

fn read_json(path: &Path) -> Result<Value, ReadError> {
    let bytes = fs::read(path).map_err(|error| ReadError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    serde_json::from_slice(&bytes).map_err(|e| ReadError::Corrupt {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

/// Writes through a temporary file in the same directory, so that the file
/// is either the old one or the new one, never half written.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .map_or_else(Default::default, |n| n.to_os_string());
    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(&name);
    temporary_name.push(".tmp");
    let temporary = path.with_file_name(temporary_name);
    let result = File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Copies the files a migrated project's first save replaces into
/// `backup/format-v<n>/`, once.
fn back_up(from: &ProjectDir) -> Result<(), SaveError> {
    let backup = from
        .path
        .join("backup")
        .join(format!("format-v{}", from.version));
    if backup.exists() {
        return Ok(());
    }
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |error| SaveError::Io { path, error }
    };
    let backup_structures = backup.join(STRUCTURES);
    fs::create_dir_all(&backup_structures).map_err(io(&backup_structures))?;
    fs::copy(from.path.join(MANIFEST), backup.join(MANIFEST)).map_err(io(&backup))?;
    let structures = from.path.join(STRUCTURES);
    for file in fs::read_dir(&structures).map_err(io(&structures))? {
        let file = file.map_err(io(&structures))?;
        fs::copy(file.path(), backup_structures.join(file.file_name()))
            .map_err(io(&file.path()))?;
    }
    Ok(())
}

/// Removes structure files and copies of imported files that the manifest
/// no longer refers to. Only files named as this crate names them are
/// touched; failures leave a file behind and are not reported.
fn remove_unreferenced(path: &Path, manifest: &Manifest) {
    let entries: HashSet<EntryId> = manifest.entries.iter().map(|e| e.id).collect();
    let copies: HashSet<String> = manifest
        .entries
        .iter()
        .filter_map(|e| e.provenance.import())
        .map(|import| import.sha256.to_string())
        .collect();
    let remove = |dir: &str, unreferenced: &dyn Fn(&str, &str) -> bool| {
        let Ok(files) = fs::read_dir(path.join(dir)) else {
            return;
        };
        for file in files.flatten() {
            let name = file.file_name();
            let Some((stem, extension)) = name.to_str().and_then(|n| n.split_once('.')) else {
                continue;
            };
            if unreferenced(stem, extension) {
                let _ = fs::remove_file(file.path());
            }
        }
    };
    remove(STRUCTURES, &|stem, extension| {
        extension == "json" && EntryId::parse(stem).is_some_and(|id| !entries.contains(&id))
    });
    remove(SOURCES, &|stem, _| {
        Sha256::is_digest(stem) && !copies.contains(stem)
    });
}

/// Why a project or one of its parts could not be read.
#[derive(Debug)]
pub enum ReadError {
    /// The directory has no project manifest.
    NotAProject(PathBuf),
    /// Written by a newer version of the application.
    Newer {
        version: u32,
    },
    Io {
        path: PathBuf,
        error: io::Error,
    },
    /// Not valid JSON, or not what the format expects.
    Corrupt {
        path: PathBuf,
        reason: String,
    },
    /// A structure that breaks the model's constraints.
    Invalid {
        path: PathBuf,
        error: StructureError,
    },
    /// The copy of an imported file is gone.
    SourceMissing(PathBuf),
    /// The copy of an imported file no longer has its content.
    SourceChanged(PathBuf),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAProject(path) => write!(f, "{} is not a project", path.display()),
            Self::Newer { version } => write!(
                f,
                "the project is in format version {version}, newer than {VERSION}"
            ),
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Corrupt { path, reason } => write!(f, "{}: {reason}", path.display()),
            Self::Invalid { path, error } => write!(f, "{}: {error}", path.display()),
            Self::SourceMissing(path) => write!(f, "{} is missing", path.display()),
            Self::SourceChanged(path) => write!(f, "{} has changed", path.display()),
        }
    }
}

impl std::error::Error for ReadError {}

/// Why a project could not be saved. Files written before the failure keep
/// their new content; the manifest is written last.
#[derive(Debug)]
pub enum SaveError {
    Io {
        path: PathBuf,
        error: io::Error,
    },
    /// An entry of an older format could not be read to migrate it.
    Read(ReadError),
    /// An entry whose content was neither given nor in a project to copy
    /// it from.
    Missing(EntryId),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Read(error) => error.fmt(f),
            Self::Missing(id) => write!(f, "entry {id} has no content to write"),
        }
    }
}

impl std::error::Error for SaveError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_save_of_a_migrated_project_backs_it_up_once() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("old.mwproj");
        fs::create_dir_all(path.join(STRUCTURES)).unwrap();
        fs::write(path.join(MANIFEST), "old manifest").unwrap();
        fs::write(path.join(STRUCTURES).join("a.json"), "old structure").unwrap();
        let from = ProjectDir {
            path: path.clone(),
            version: 0,
        };
        back_up(&from).unwrap();
        let backup = path.join("backup/format-v0");
        assert_eq!(
            fs::read_to_string(backup.join(MANIFEST)).unwrap(),
            "old manifest"
        );
        assert_eq!(
            fs::read_to_string(backup.join("structures/a.json")).unwrap(),
            "old structure"
        );
        // A later save does not overwrite the backup.
        fs::write(path.join(MANIFEST), "new manifest").unwrap();
        back_up(&from).unwrap();
        assert_eq!(
            fs::read_to_string(backup.join(MANIFEST)).unwrap(),
            "old manifest"
        );
    }

    #[test]
    fn atomic_writes_leave_no_temporary_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file.json");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        assert!(write_atomic(&temp.path().join("missing/file.json"), b"x").is_err());
    }
}
