//! The host without a window (ai-integration design §5.1): projects read
//! from their files, and CIF files outside any project, all read-only.
//! Nothing is ever written: not the projects, not their view state, not the
//! application's list of recent projects.

use std::sync::Arc;

use mw_automation::{
    CallError, EntryInfo, Host, OpenedFile, Opening, ProjectInfo, Projects, RecentProject,
    RecentProjects, SkippedBlock, StructureEntryId, StructureInfo,
};
use mw_crystal::CrystalDocument;
use mw_crystal_io::{ImportedBlock, cif};
use mw_project_file::{
    EntryId, EntryKind, Import, ProjectDir, ProjectId, Provenance, ReadError, Sha256, SourceFormat,
    Timestamp, read_manifest, structure_sha256,
};

use crate::access::Access;

/// The name of the project that holds files opened outside any project.
const UNTITLED: &str = "Opened files";

/// Where the recent projects come from: the application's configuration
/// directory, read again on every call (the application may open projects
/// meanwhile). The standalone server has none.
pub type RecentSource = Box<dyn Fn() -> Vec<RecentProject> + Send>;

struct Entry {
    id: StructureEntryId,
    entry: mw_project_file::Entry,
    /// Loaded by `structure.open`.
    document: Option<CrystalDocument>,
    /// For an imported structure whose file's copy could be read.
    source: Option<Arc<ImportedBlock>>,
}

struct Project {
    info: ProjectInfo,
    /// `None` for the untitled project.
    dir: Option<ProjectDir>,
    entries: Vec<Entry>,
}

/// A file opened outside any project, to open it again by the same ids.
struct OpenedRecord {
    sha256: Sha256,
    file: OpenedFile,
}

/// Projects and files a client opened, read-only (design §5.1).
pub struct Headless {
    access: Access,
    recent: RecentSource,
    projects: Vec<Project>,
    files: Vec<OpenedRecord>,
    last_id: u32,
}

impl Headless {
    pub fn new(access: Access) -> Self {
        Self {
            access,
            recent: Box::new(Vec::new),
            projects: Vec::new(),
            files: Vec::new(),
            last_id: 0,
        }
    }

    /// Lists recent projects from `source`.
    pub fn with_recent(mut self, source: RecentSource) -> Self {
        self.recent = source;
        self
    }

    pub fn access(&self) -> &Access {
        &self.access
    }

    fn next_id(&mut self) -> StructureEntryId {
        self.last_id += 1;
        StructureEntryId(self.last_id)
    }

    fn entries_iter(&self) -> impl Iterator<Item = (&Project, &Entry)> {
        self.projects
            .iter()
            .flat_map(|project| project.entries.iter().map(move |entry| (project, entry)))
    }

    fn entry(&self, id: StructureEntryId) -> Option<&Entry> {
        self.entries_iter()
            .map(|(_, entry)| entry)
            .find(|entry| entry.id == id)
    }

    /// The project for files opened outside any project, made on first use.
    fn untitled(&mut self) -> &mut Project {
        let index = match self.projects.iter().position(|p| p.dir.is_none()) {
            Some(index) => index,
            None => {
                self.projects.push(Project {
                    info: ProjectInfo::new(ProjectId::new(), UNTITLED, None),
                    dir: None,
                    entries: Vec::new(),
                });
                self.projects.len() - 1
            }
        };
        &mut self.projects[index]
    }
}

/// Why a project or one of its structures could not be read, for a model.
fn read_error(error: ReadError) -> CallError {
    let code = match &error {
        ReadError::NotAProject(_) => "not_a_project",
        ReadError::Newer { .. } => "newer_format",
        ReadError::Io { .. } => "io_error",
        _ => "corrupt_project",
    };
    let hint = match &error {
        ReadError::NotAProject(_) => {
            ": projects are directories named name.mwproj with a project.json"
        }
        ReadError::Newer { .. } => "; a newer Materials Workbench reads it",
        _ => "",
    };
    CallError::new(code, format!("{error}{hint}"))
}

/// Reads a structure of a project and, for an imported one, the data block
/// it came from again, as the interface opens it (design §7.1 of the
/// project format). Blocks on file I/O and symmetry analysis.
fn read_structure(
    dir: &ProjectDir,
    entry: &mw_project_file::Entry,
) -> Result<(CrystalDocument, Option<Arc<ImportedBlock>>), CallError> {
    let file = dir.read_structure(entry.id).map_err(read_error)?;
    let source = entry.provenance.import().and_then(|import| {
        let bytes = dir.read_source(import).ok()?;
        let mut block = import.read_block(&bytes)?;
        // A CIF export writes the source file's other fields back only while
        // the structure is the one imported. The project records that by
        // hash, as the interface decides it; the block's own structure,
        // which the export compares with, is made to say the same.
        block.structure = (structure_sha256(&file.structure) == import.structure_sha256)
            .then(|| file.structure.clone());
        Some(Arc::new(block))
    });
    Ok((
        CrystalDocument::restore(file.structure, file.measurements),
        source,
    ))
}

impl Host for Headless {
    fn structures(&self) -> Vec<StructureInfo> {
        self.entries_iter()
            .filter(|(_, entry)| entry.document.is_some())
            .map(|(project, entry)| {
                StructureInfo::new(entry.id, &entry.entry.name, &project.info.name)
            })
            .collect()
    }

    fn document(&self, id: StructureEntryId) -> Option<&CrystalDocument> {
        self.entry(id)?.document.as_ref()
    }

    fn document_mut(&mut self, id: StructureEntryId) -> Option<&mut CrystalDocument> {
        self.projects
            .iter_mut()
            .flat_map(|project| project.entries.iter_mut())
            .find(|entry| entry.id == id)?
            .document
            .as_mut()
    }

    fn source(&self, id: StructureEntryId) -> Option<&ImportedBlock> {
        self.entry(id)?.source.as_deref()
    }

    fn projects(&self) -> Option<&dyn Projects> {
        Some(self)
    }

    fn projects_mut(&mut self) -> Option<&mut dyn Projects> {
        Some(self)
    }
}

impl Projects for Headless {
    fn projects(&self) -> Vec<ProjectInfo> {
        self.projects.iter().map(|p| p.info.clone()).collect()
    }

    fn recent_projects(&self) -> RecentProjects {
        let (projects, withheld): (Vec<_>, Vec<_>) = (self.recent)()
            .into_iter()
            .partition(|project| self.access.permits(&project.path));
        RecentProjects {
            projects,
            withheld: withheld.len(),
        }
    }

    fn entries(&self) -> Vec<EntryInfo> {
        self.entries_iter()
            .map(|(project, entry)| {
                EntryInfo::new(
                    entry.id,
                    project.info.id,
                    entry.entry.id,
                    &entry.entry.name,
                    entry.entry.provenance.clone(),
                    entry.document.is_some(),
                )
            })
            .collect()
    }

    fn open_project(&mut self, path: &str) -> Result<(ProjectId, Opening), CallError> {
        let path = self.access.project(path)?;
        if let Some(project) = self
            .projects
            .iter()
            .find(|p| p.info.path.as_ref() == Some(&path))
        {
            return Ok((project.info.id, Opening::AlreadyOpen));
        }
        let (dir, manifest) = read_manifest(&path).map_err(read_error)?;
        if let Some(open) = self.projects.iter().find(|p| p.info.id == manifest.id) {
            let elsewhere = open
                .info
                .path
                .as_ref()
                .map_or_else(String::new, |p| format!(" from {}", p.display()));
            return Err(CallError::new(
                "duplicate_project",
                format!(
                    "{} is a copy of the open project {}{elsewhere}: both have the id {}. Only \
                     one of them can be open.",
                    path.display(),
                    open.info.name,
                    manifest.id
                ),
            ));
        }
        let entries = manifest
            .entries
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::CrystalStructure)
            .map(|entry| Entry {
                id: self.next_id(),
                entry,
                document: None,
                source: None,
            })
            .collect();
        let id = manifest.id;
        self.projects.push(Project {
            info: ProjectInfo::new(id, manifest.name, Some(path)),
            dir: Some(dir),
            entries,
        });
        Ok((id, Opening::Opened))
    }

    fn open_structure(&mut self, id: StructureEntryId) -> Result<Opening, CallError> {
        let (project, index) = self
            .projects
            .iter()
            .enumerate()
            .find_map(|(p, project)| {
                let index = project.entries.iter().position(|e| e.id == id)?;
                Some((p, index))
            })
            .ok_or_else(|| {
                CallError::new(
                    "unknown_structure",
                    format!(
                        "no structure with id {}; structure.list gives the ids of the open \
                         projects' structures",
                        id.0
                    ),
                )
            })?;
        let project = &mut self.projects[project];
        let entry = &project.entries[index];
        if entry.document.is_some() {
            return Ok(Opening::AlreadyOpen);
        }
        let dir = project
            .dir
            .as_ref()
            .expect("structures of the untitled project are open from the start");
        let (document, source) = read_structure(dir, &entry.entry)?;
        let entry = &mut project.entries[index];
        entry.document = Some(document);
        entry.source = source;
        Ok(Opening::Opened)
    }

    fn open_file(&mut self, path: &str) -> Result<(OpenedFile, Opening), CallError> {
        let (path, bytes) = self.access.read(path)?;
        let sha256 = Sha256::of(&bytes);
        if let Some(record) = self
            .files
            .iter()
            .find(|f| f.file.path == path && f.sha256 == sha256)
        {
            return Ok((record.file.clone(), Opening::AlreadyOpen));
        }
        let report = cif::read(&bytes);
        let skipped = report
            .failed_blocks()
            .map(|block| SkippedBlock::new(&block.name, block.line, block.diagnostics.clone()))
            .collect();
        let file_name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let stem = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let several = report.structures().count() > 1;
        let now = Timestamp::now();
        let mut structures = Vec::new();
        for (block, structure) in report.structures() {
            // Named and with the file's diagnostics, as the interface opens
            // files (mw-structure/src/import.rs).
            let name = if several {
                format!("{stem} · {}", block.name)
            } else {
                stem.clone()
            };
            let mut block = block.clone();
            block
                .diagnostics
                .splice(0..0, report.diagnostics.iter().cloned());
            let import = Import::new(
                SourceFormat::Cif,
                &file_name,
                Some(path.display().to_string()),
                sha256.clone(),
                &block.name,
                now.clone(),
                structure_sha256(structure),
            );
            let entry = mw_project_file::Entry::new(
                EntryId::new(),
                EntryKind::CrystalStructure,
                name,
                Provenance::Import(import),
            );
            let id = self.next_id();
            structures.push(id);
            let document = CrystalDocument::new(structure.clone());
            self.untitled().entries.push(Entry {
                id,
                entry,
                document: Some(document),
                source: Some(Arc::new(block)),
            });
        }
        let file = OpenedFile::new(path, structures, skipped, report.diagnostics);
        if !file.structures.is_empty() {
            self.files.push(OpenedRecord {
                sha256,
                file: file.clone(),
            });
        }
        Ok((file, Opening::Opened))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use mw_automation::Registry;
    use serde_json::{Value, json};

    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    fn call(host: &mut Headless, name: &str, input: Value) -> Result<Value, CallError> {
        Registry::standard()
            .call(host, name, input)
            .map(|result| result.output)
    }

    /// The sample projects, installed in a directory the host allows.
    fn samples() -> (tempfile::TempDir, Headless, Vec<PathBuf>) {
        let temp = tempfile::tempdir().unwrap();
        let installed = mw_project_file::samples::install(temp.path()).unwrap();
        let host = Headless::new(Access::new(&[temp.path().to_path_buf()]).unwrap());
        (temp, host, installed)
    }

    #[test]
    fn projects_open_without_writing() {
        let (temp, mut host, installed) = samples();
        let tio2 = installed[0].to_str().unwrap();
        let before = std::fs::read_dir(&installed[0]).unwrap().count();
        let project = call(&mut host, "project.open", json!({ "path": tio2 })).unwrap();
        assert_eq!(project["name"], "TiO₂ 相研究");
        let structures = project["structures"].as_array().unwrap();
        assert_eq!(structures.len(), 2);
        assert_eq!(structures[1]["origin"], "2×2×2 supercell of 金红石");
        let rutile = structures[0]["structure"].clone();

        let opened = call(&mut host, "structure.open", json!({ "structure": rutile })).unwrap();
        assert_eq!(opened["note"], Value::Null);
        // The copy of the imported file was read again: the export writes
        // its other fields back, as for an unchanged structure.
        let cif = call(
            &mut host,
            "structure.export",
            json!({ "structure": rutile, "format": "cif" }),
        )
        .unwrap();
        assert_eq!(cif["dropped_fields"], json!([]));
        assert!(
            cif["text"]
                .as_str()
                .unwrap()
                .contains("_chemical_name_mineral"),
            "{}",
            cif["text"]
        );

        // Nothing was written.
        assert_eq!(std::fs::read_dir(&installed[0]).unwrap().count(), before);
        let copy = temp.path().join("copy.mwproj");
        std::fs::create_dir(&copy).unwrap();
        std::fs::copy(installed[0].join("project.json"), copy.join("project.json")).unwrap();
        let duplicate = call(&mut host, "project.open", json!({ "path": copy }));
        assert_eq!(duplicate.unwrap_err().code, "duplicate_project");
    }

    #[test]
    fn a_missing_copy_of_the_source_is_no_error() {
        let (_temp, mut host, installed) = samples();
        let sources = installed[1].join("sources");
        for file in std::fs::read_dir(&sources).unwrap() {
            std::fs::remove_file(file.unwrap().path()).unwrap();
        }
        call(&mut host, "project.open", json!({ "path": installed[1] })).unwrap();
        let opened = call(&mut host, "structure.open", json!({ "structure": 1 })).unwrap();
        assert!(
            opened["note"].as_str().unwrap().contains("SrTiO3.cif"),
            "{opened}"
        );
        let diagnostics = json!({ "structure": 1, "offset": 0, "limit": 10 });
        let diagnostics = call(&mut host, "structure.diagnostics", diagnostics).unwrap();
        assert_eq!(diagnostics["block"], Value::Null);
    }

    #[test]
    fn unreadable_projects_are_refused() {
        let (temp, mut host, _) = samples();
        let empty = temp.path().join("empty.mwproj");
        std::fs::create_dir(&empty).unwrap();
        let open = |host: &mut Headless, path: &Path| {
            call(host, "project.open", json!({ "path": path }))
                .unwrap_err()
                .code
        };
        assert_eq!(open(&mut host, &empty), "not_a_project");
        std::fs::write(
            empty.join("project.json"),
            r#"{"format": "materials-workbench/project", "version": 99}"#,
        )
        .unwrap();
        assert_eq!(open(&mut host, &empty), "newer_format");
        assert_eq!(open(&mut host, &fixtures()), "not_allowed");
    }

    #[test]
    fn recent_projects_outside_the_allowed_directories_are_withheld() {
        let (temp, host, installed) = samples();
        let recent = |path: PathBuf| {
            RecentProject::new(ProjectId::new(), "p", path, Timestamp::from_unix(0))
        };
        let inside = recent(installed[0].clone());
        let outside = recent(temp.path().join("../elsewhere.mwproj"));
        let projects = vec![inside.clone(), outside];
        let mut host = host.with_recent(Box::new(move || projects.clone()));
        let list = call(&mut host, "project.list", json!({})).unwrap();
        assert_eq!(list["recent"].as_array().unwrap().len(), 1);
        assert_eq!(
            list["recent"][0]["path"],
            installed[0].display().to_string()
        );
        assert_eq!(list["recent_elsewhere"], 1);
    }
}
