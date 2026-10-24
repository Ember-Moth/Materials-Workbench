//! The host without a window (ai-integration design §5.1, §5.3): projects
//! read from their files, and files outside any project. Writes go back to
//! the project's files at once: the first one takes the project's lock and
//! checks that no one saved the project since it was read, and every file
//! replaced is backed up first (project format design §15). A session that
//! only reads writes nothing: not the projects, not their view state, not
//! the application's list of recent projects.

use std::path::PathBuf;
use std::sync::Arc;

use mw_automation::{
    CallError, Client, EntryInfo, Host, OpenedFile, Opening, Permissions, ProjectInfo, Projects,
    RecentProject, RecentProjects, StructureEntryId, StructureInfo, duplicate_project,
    parse_import, read_error, supercell_name, unknown_structure,
};
use mw_crystal::{Agent, Command, CrystalDocument, Outcome, Structure};
use mw_crystal_io::ImportedBlock;
use mw_project_file::{
    AuditRecord, Derived, EntryId, EntryKind, LockError, Manifest, Operation, ProjectDir,
    ProjectId, ProjectLock, Provenance, Save, SessionBackup, Sha256, Source, StructureFile,
    Timestamp, append_audit, read_manifest, structure_sha256,
};

use crate::access::Access;

/// The name of the project that holds files opened outside any project.
const UNTITLED: &str = "Opened files";
/// How the project lock names this process.
const HOLDER: &str = "materials-workbench mcp";

/// Where the recent projects come from: the application's configuration
/// directory, read again on every call (the application may open projects
/// meanwhile). The standalone server has none.
pub type RecentSource = Box<dyn Fn() -> Vec<RecentProject> + Send>;

/// What the user lets a client do: the application's settings, read again
/// on every call (design §7.2). The standalone server has the defaults.
pub type PermissionSource = Box<dyn Fn(&Client) -> Permissions + Send>;

struct Entry {
    id: StructureEntryId,
    entry: mw_project_file::Entry,
    /// Loaded by `structure.open`.
    document: Option<CrystalDocument>,
    /// For an imported structure whose file's copy could be read.
    source: Option<Arc<ImportedBlock>>,
    /// The commands of earlier sessions, from the structure file; this
    /// session's follow them when saving (project format design §7.3).
    edits: Vec<Command>,
    /// The structure file's digest when it was read, to tell whether
    /// another process saved it since.
    digest: Option<Sha256>,
    /// Not in the project's files yet: imported or derived in this session.
    new: bool,
}

impl Entry {
    fn opened(
        id: StructureEntryId,
        entry: mw_project_file::Entry,
        structure: Structure,
        source: Option<Arc<ImportedBlock>>,
        new: bool,
    ) -> Self {
        Self {
            id,
            entry,
            document: Some(CrystalDocument::new(structure)),
            source,
            edits: Vec::new(),
            digest: None,
            new,
        }
    }

    /// What its structure file is to hold.
    fn file(&self) -> Option<StructureFile> {
        let document = self.document.as_ref()?;
        Some(StructureFile::new(
            document.structure().clone(),
            document.annotations().to_vec(),
            self.edits
                .iter()
                .chain(document.history())
                .cloned()
                .collect(),
        ))
    }

    fn is_modified(&self) -> bool {
        self.new || self.document.as_ref().is_some_and(|d| d.is_modified())
    }
}

struct Project {
    info: ProjectInfo,
    /// `None` for the untitled project, which is never saved.
    dir: Option<ProjectDir>,
    entries: Vec<Entry>,
    /// The manifest's digest when it was read.
    digest: Option<Sha256>,
    /// Copies of files imported in this session, until they are saved.
    sources: Vec<Source>,
    /// Held from the first write on, until the process ends.
    lock: Option<ProjectLock>,
    backup: Option<SessionBackup>,
    /// Records of calls not yet in the project's audit log.
    audit: Vec<AuditRecord>,
}

impl Project {
    fn new(info: ProjectInfo, dir: Option<ProjectDir>, entries: Vec<Entry>) -> Self {
        let digest = dir.as_ref().and_then(ProjectDir::manifest_digest);
        Self {
            info,
            dir,
            entries,
            digest,
            sources: Vec::new(),
            lock: None,
            backup: None,
            audit: Vec::new(),
        }
    }

    fn manifest(&self) -> Manifest {
        let mut manifest = Manifest::new(&self.info.name);
        manifest.id = self.info.id;
        manifest.entries = self.entries.iter().map(|e| e.entry.clone()).collect();
        manifest
    }

    /// Makes sure this process may write the project: takes its lock and
    /// checks that no one saved the project since it was read (project
    /// format design §15). The untitled project needs neither.
    fn writable(&mut self) -> Result<(), CallError> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        if self.lock.is_some() {
            return Ok(());
        }
        let lock = ProjectLock::acquire(dir.path(), HOLDER).map_err(|e| locked(&self.info, e))?;
        let saved_since = dir.manifest_digest() != self.digest
            || self.entries.iter().any(|entry| {
                entry.digest.is_some() && dir.structure_digest(entry.entry.id) != entry.digest
            });
        if saved_since {
            return Err(CallError::new(
                "project_changed",
                format!(
                    "{} was saved by another program after this server read it, so nothing \
                     was changed. Start the client's connection to the server again to read the \
                     project anew.",
                    self.info.name
                ),
            ));
        }
        self.backup = Some(SessionBackup::new(dir.path()));
        self.lock = Some(lock);
        Ok(())
    }

    /// Writes what changed since the last save: new copies of imported
    /// files, changed structures, the manifest last; backs up the files it
    /// replaces first. Only after [`Self::writable`].
    fn save(&mut self) -> Result<(), CallError> {
        let Some(dir) = self.dir.clone() else {
            return Ok(());
        };
        let failed = |reason: String| {
            CallError::new(
                "save_failed",
                format!(
                    "{} could not be saved, so the change was taken back: {reason}",
                    self.info.name
                ),
            )
        };
        let save = Save {
            manifest: self.manifest(),
            structures: self
                .entries
                .iter()
                .filter(|entry| entry.is_modified())
                .filter_map(|entry| Some((entry.entry.id, entry.file()?)))
                .collect(),
            sources: self.sources.clone(),
            from: Some(dir.clone()),
            view: None,
        };
        let backup = self.backup.as_mut().expect("written only once writable");
        backup.protect(&save).map_err(|e| failed(e.to_string()))?;
        let saved = mw_project_file::save(dir.path(), &save).map_err(|e| failed(e.to_string()))?;
        self.digest = saved.manifest_digest();
        for entry in &mut self.entries {
            if let Some(document) = &mut entry.document {
                let checkpoint = document.checkpoint();
                document.mark_saved(checkpoint);
                entry.digest = saved.structure_digest(entry.entry.id);
            }
            entry.new = false;
        }
        self.dir = Some(saved);
        self.sources.clear();
        self.write_audit();
        Ok(())
    }

    /// Appends the records waiting for the audit log, once this process
    /// writes the project.
    fn write_audit(&mut self) {
        let (Some(dir), Some(_)) = (&self.dir, &self.lock) else {
            return;
        };
        if let Err(error) = append_audit(dir.path(), &self.audit) {
            log::warn!(
                "could not append to the audit log of {}: {error}",
                self.info.name
            );
        }
        self.audit.clear();
    }

    /// A free name for a new structure, as the interface makes them:
    /// `name`, `name 2`, `name 3`, …
    fn free_name(&self, name: &str) -> String {
        let taken = |candidate: &str| self.entries.iter().any(|e| e.entry.name == candidate);
        (1..)
            .map(|n| {
                if n == 1 {
                    name.to_owned()
                } else {
                    format!("{name} {n}")
                }
            })
            .find(|candidate| !taken(candidate))
            .expect("some name is free")
    }
}

/// The refusal of a write to a project another process writes.
fn locked(project: &ProjectInfo, error: LockError) -> CallError {
    match error {
        LockError::Held(holder) => {
            let who = holder.map_or_else(
                || "another program is writing it".to_owned(),
                |holder| format!("{holder} is writing it"),
            );
            CallError::new(
                "project_locked",
                format!(
                    "{} cannot be changed now: {who}. Nothing was changed. When the project is \
                     open in Materials Workbench, the user can let MCP clients connect to the \
                     application (Settings) and start this client's connection again, so that \
                     it edits the project there; or close the project in the application.",
                    project.name
                ),
            )
        }
        LockError::Io(error) => CallError::new(
            "io_error",
            format!("{} cannot be locked for writing: {error}", project.name),
        ),
    }
}

/// A file opened outside any project, to open it again by the same ids.
struct OpenedRecord {
    sha256: Sha256,
    file: OpenedFile,
}

/// Projects and files a client opened (design §5.1).
pub struct Headless {
    access: Access,
    recent: RecentSource,
    permissions: PermissionSource,
    projects: Vec<Project>,
    files: Vec<OpenedRecord>,
    last_id: u32,
}

impl Headless {
    pub fn new(access: Access) -> Self {
        Self {
            access,
            recent: Box::new(Vec::new),
            permissions: Box::new(|_| Permissions::default()),
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

    /// Takes what clients may do from `source`.
    pub fn with_permissions(mut self, source: PermissionSource) -> Self {
        self.permissions = source;
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

    /// The project and the index of a structure in it.
    fn locate(&self, id: StructureEntryId) -> Result<(usize, usize), CallError> {
        self.projects
            .iter()
            .enumerate()
            .find_map(|(p, project)| {
                let index = project.entries.iter().position(|e| e.id == id)?;
                Some((p, index))
            })
            .ok_or_else(|| unknown_structure(id))
    }

    /// The open projects' directories, which clients may read and write in
    /// as in the allowed directories (design §7.2).
    fn project_dirs(&self) -> Vec<PathBuf> {
        self.projects
            .iter()
            .filter_map(|p| p.info.path.clone())
            .collect()
    }

    /// Changes an open structure through `change`, then saves its project;
    /// `undo` takes the change back when saving fails.
    fn change<T>(
        &mut self,
        id: StructureEntryId,
        change: impl FnOnce(&mut CrystalDocument) -> Result<(T, bool), CallError>,
        undo: impl FnOnce(&mut CrystalDocument),
    ) -> Result<T, CallError> {
        let (p, index) = self.locate(id)?;
        let project = &mut self.projects[p];
        project.writable()?;
        let document = project.entries[index]
            .document
            .as_mut()
            .ok_or_else(|| unknown_structure(id))?;
        let (result, changed) = change(document)?;
        if changed && let Err(error) = project.save() {
            if let Some(document) = project.entries[index].document.as_mut() {
                undo(document);
            }
            return Err(error);
        }
        Ok(result)
    }

    /// The project for files opened outside any project, made on first use.
    fn untitled(&mut self) -> &mut Project {
        let index = match self.projects.iter().position(|p| p.dir.is_none()) {
            Some(index) => index,
            None => {
                let info = ProjectInfo::new(ProjectId::new(), UNTITLED, None);
                self.projects.push(Project::new(info, None, Vec::new()));
                self.projects.len() - 1
            }
        };
        &mut self.projects[index]
    }
}

/// Reads a structure of a project and, for an imported one, the data block
/// it came from again, as the interface opens it (design §7.1 of the
/// project format). Blocks on file I/O and symmetry analysis.
fn read_structure(dir: &ProjectDir, entry: &mut Entry) -> Result<(), CallError> {
    let file = dir.read_structure(entry.entry.id).map_err(read_error)?;
    entry.digest = dir.structure_digest(entry.entry.id);
    entry.source = entry.entry.provenance.import().and_then(|import| {
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
    entry.document = Some(CrystalDocument::restore(file.structure, file.measurements));
    entry.edits = file.edits;
    Ok(())
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

    fn execute(
        &mut self,
        id: StructureEntryId,
        command: Command,
        agent: Option<Agent>,
    ) -> Result<Outcome, CallError> {
        self.change(
            id,
            |document| {
                let outcome = document.execute_by(command, agent)?;
                let changed = outcome.changed;
                Ok((outcome, changed))
            },
            |document| {
                document.undo();
            },
        )
    }

    fn undo(
        &mut self,
        id: StructureEntryId,
        _: Option<Agent>,
    ) -> Result<Option<Outcome>, CallError> {
        self.change(
            id,
            |document| {
                let outcome = document.undo();
                let changed = outcome.is_some();
                Ok((outcome, changed))
            },
            |document| {
                document.redo();
            },
        )
    }

    fn redo(
        &mut self,
        id: StructureEntryId,
        _: Option<Agent>,
    ) -> Result<Option<Outcome>, CallError> {
        self.change(
            id,
            |document| {
                let outcome = document.redo();
                let changed = outcome.is_some();
                Ok((outcome, changed))
            },
            |document| {
                document.undo();
            },
        )
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

    /// Records go to the project's audit log once this process writes the
    /// project; until then they wait, so that a session that only reads
    /// writes nothing (design §5.3). The server's log gets every record.
    fn audit(&mut self, project: Option<ProjectId>, record: AuditRecord) {
        log::info!("audit: {}", record.to_line());
        let Some(project) =
            project.and_then(|id| self.projects.iter_mut().find(|p| p.info.id == id))
        else {
            return;
        };
        if project.dir.is_some() {
            project.audit.push(record);
            project.write_audit();
        }
    }

    fn writable(&self, path: &str) -> Result<PathBuf, CallError> {
        self.access.writable(path, &self.project_dirs())
    }

    fn permissions(&self, client: &Client) -> Permissions {
        (self.permissions)(client)
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
            return Err(duplicate_project(&path, &open.info));
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
                edits: Vec::new(),
                digest: None,
                new: false,
            })
            .collect();
        let id = manifest.id;
        let info = ProjectInfo::new(id, manifest.name, Some(path));
        self.projects.push(Project::new(info, Some(dir), entries));
        Ok((id, Opening::Opened))
    }

    fn open_structure(&mut self, id: StructureEntryId) -> Result<Opening, CallError> {
        let (p, index) = self.locate(id)?;
        let project = &mut self.projects[p];
        let entry = &mut project.entries[index];
        if entry.document.is_some() {
            return Ok(Opening::AlreadyOpen);
        }
        let dir = project
            .dir
            .as_ref()
            .expect("structures of the untitled project are open from the start");
        read_structure(dir, entry)?;
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
        let parsed = parse_import(&path, bytes)?;
        let now = Timestamp::now();
        let mut structures = Vec::new();
        for (name, block) in &parsed.structures {
            let import = parsed.import(block, now.clone());
            let entry = mw_project_file::Entry::new(
                EntryId::new(),
                EntryKind::CrystalStructure,
                name,
                Provenance::Import(import),
            );
            let id = self.next_id();
            structures.push(id);
            let structure = block.structure.clone().expect("imported blocks hold one");
            let source = Some(Arc::new(block.clone()));
            self.untitled()
                .entries
                .push(Entry::opened(id, entry, structure, source, false));
        }
        let file = OpenedFile::new(path, structures, parsed.skipped, parsed.diagnostics);
        if !file.structures.is_empty() {
            self.files.push(OpenedRecord {
                sha256,
                file: file.clone(),
            });
        }
        Ok((file, Opening::Opened))
    }

    fn import_file(&mut self, project: ProjectId, path: &str) -> Result<OpenedFile, CallError> {
        let p = self
            .projects
            .iter()
            .position(|p| p.info.id == project)
            .ok_or_else(|| {
                CallError::new(
                    "unknown_project",
                    format!("no open project has the id {project}; project.list lists them"),
                )
            })?;
        let (path, bytes) = self.access.read_in(path, &self.project_dirs())?;
        let parsed = parse_import(&path, bytes)?;
        if parsed.structures.is_empty() {
            return Ok(OpenedFile::new(
                path,
                Vec::new(),
                parsed.skipped,
                parsed.diagnostics,
            ));
        }
        self.projects[p].writable()?;
        let ids: Vec<StructureEntryId> = parsed.structures.iter().map(|_| self.next_id()).collect();
        let project = &mut self.projects[p];
        let before = project.entries.len();
        let now = Timestamp::now();
        for ((name, block), &id) in parsed.structures.iter().zip(&ids) {
            let import = parsed.import(block, now.clone());
            let entry = mw_project_file::Entry::new(
                EntryId::new(),
                EntryKind::CrystalStructure,
                name,
                Provenance::Import(import),
            );
            let structure = block.structure.clone().expect("imported blocks hold one");
            let source = Some(Arc::new(block.clone()));
            project
                .entries
                .push(Entry::opened(id, entry, structure, source, true));
        }
        project.sources.push(Source {
            sha256: parsed.sha256.clone(),
            format: parsed.format,
            bytes: parsed.bytes.clone(),
        });
        if let Err(error) = project.save() {
            project.entries.truncate(before);
            project.sources.clear();
            return Err(error);
        }
        Ok(OpenedFile::new(
            path,
            ids,
            parsed.skipped,
            parsed.diagnostics,
        ))
    }

    fn add_derived(
        &mut self,
        from: StructureEntryId,
        name: Option<&str>,
        structure: Structure,
        matrix: [[i32; 3]; 3],
    ) -> Result<StructureEntryId, CallError> {
        let (p, index) = self.locate(from)?;
        let source_sha256 = self.projects[p].entries[index]
            .document
            .as_ref()
            .map(|document| structure_sha256(document.structure()))
            .ok_or_else(|| unknown_structure(from))?;
        self.projects[p].writable()?;
        let id = self.next_id();
        let project = &mut self.projects[p];
        let source = project.entries[index].entry.clone();
        let name = name.map_or_else(|| supercell_name(&source.name, matrix), str::to_owned);
        let name = project.free_name(&name);
        let record = Derived::new(
            source.id,
            source.name,
            source_sha256,
            Operation::MakeSupercell { matrix },
            Timestamp::now(),
        );
        let entry = mw_project_file::Entry::new(
            EntryId::new(),
            EntryKind::CrystalStructure,
            name,
            Provenance::Derived(record),
        );
        // After its source and what was made from it before (project format
        // design §13.2).
        let after = project.entries[index + 1..]
            .iter()
            .take_while(
                |e| matches!(&e.entry.provenance, Provenance::Derived(d) if d.from == source.id),
            )
            .count();
        let position = index + 1 + after;
        project
            .entries
            .insert(position, Entry::opened(id, entry, structure, None, true));
        if let Err(error) = project.save() {
            project.entries.remove(position);
            return Err(error);
        }
        Ok(id)
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

    /// The sample project `tio2.mwproj`'s ids.
    const RUTILE: &str = "6d2a51c0-93e4-4b7a-8f10-2c4e5a6b7c01";

    fn claude() -> mw_automation::Context {
        let client = Client::new("claude-code", Some("Claude Code".into()));
        mw_automation::Context::client(client, Permissions::default())
    }

    /// Calls as an AI client.
    fn call_as(host: &mut Headless, name: &str, input: Value) -> Result<Value, CallError> {
        Registry::standard()
            .call_as(host, &claude(), name, input)
            .map(|result| result.output)
    }

    /// The sample project TiO₂ open, with its rutile (structure 1).
    fn tio2() -> (tempfile::TempDir, Headless, PathBuf) {
        let (temp, mut host, installed) = samples();
        let path = installed[0].canonicalize().unwrap();
        call_as(&mut host, "project.open", json!({ "path": path })).unwrap();
        call_as(&mut host, "structure.open", json!({ "structure": 1 })).unwrap();
        (temp, host, path)
    }

    fn set_a(a: f64) -> Value {
        json!({ "structure": 1, "values": [{ "parameter": "a", "value": a }] })
    }

    fn rutile_file(project: &Path) -> String {
        std::fs::read_to_string(project.join(format!("structures/{RUTILE}.json"))).unwrap()
    }

    /// The structure as a new session reads it from the project.
    fn reopened_a(project: &Path) -> f64 {
        let (dir, _) = read_manifest(project).unwrap();
        let file = dir.read_structure(EntryId::parse(RUTILE).unwrap()).unwrap();
        file.structure.lattice.params().a.value()
    }

    #[test]
    fn reading_writes_nothing_even_for_clients() {
        let (_temp, mut host, project) = tio2();
        call_as(&mut host, "structure.summary", json!({ "structure": 1 })).unwrap();
        // No lock, no log, no backup: the session only read.
        for file in [".lock", "audit.jsonl", "backup"] {
            assert!(!project.join(file).exists(), "{file}");
        }
    }

    #[test]
    fn edits_are_saved_at_once_after_a_backup() {
        let (_temp, mut host, project) = tio2();
        let before = rutile_file(&project);
        let manifest = std::fs::read_to_string(project.join("project.json")).unwrap();
        call_as(&mut host, "structure.set_cell", set_a(4.7)).unwrap();
        assert_eq!(reopened_a(&project), 4.7);
        let (dir, _) = read_manifest(&project).unwrap();
        let file = dir.read_structure(EntryId::parse(RUTILE).unwrap()).unwrap();
        assert_eq!(file.edits.len(), 1, "the edit record grows");

        // The files it replaced, as they were before the session.
        let backups: Vec<PathBuf> = std::fs::read_dir(project.join("backup"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(backups.len(), 1);
        let backup = &backups[0];
        assert_eq!(rutile_file(backup), before);
        let backed_up = std::fs::read_to_string(backup.join("project.json")).unwrap();
        assert_eq!(backed_up, manifest);
        // The supercell was not touched, so it was not copied.
        assert_eq!(
            std::fs::read_dir(backup.join("structures"))
                .unwrap()
                .count(),
            1
        );

        // Undo saves too; the backup keeps the first state.
        call_as(&mut host, "structure.set_cell", set_a(4.8)).unwrap();
        call_as(&mut host, "history.undo", json!({ "structure": 1 })).unwrap();
        assert_eq!(reopened_a(&project), 4.7);
        assert_eq!(rutile_file(backup), before);

        // The audit log has the client's calls, from before the first write
        // too.
        let log = mw_project_file::read_audit(&project).unwrap();
        let calls: Vec<&str> = log.iter().map(|r| r.capability.as_str()).collect();
        assert_eq!(
            calls,
            [
                "project.open",
                "structure.open",
                "structure.set_cell",
                "structure.set_cell",
                "history.undo"
            ]
        );
        assert_eq!(log[2].client.title.as_deref(), Some("Claude Code"));
        assert_eq!(log[2].structure, EntryId::parse(RUTILE));
        // The undo step is the client's.
        let document = host.document(StructureEntryId(1)).unwrap();
        assert_eq!(document.undo_agent().map(Agent::name), Some("Claude Code"));
    }

    #[test]
    fn a_project_another_process_writes_is_refused() {
        let (_temp, mut host, project) = tio2();
        let app = ProjectLock::acquire(&project, "Materials Workbench").unwrap();
        let refused = call_as(&mut host, "structure.set_cell", set_a(4.7)).unwrap_err();
        assert_eq!(refused.code, "project_locked");
        assert!(refused.message.contains("Settings"), "{}", refused.message);
        assert_eq!(host.document(StructureEntryId(1)).unwrap().revision(), 0);
        drop(app);
        call_as(&mut host, "structure.set_cell", set_a(4.7)).unwrap();
    }

    #[test]
    fn a_project_saved_since_it_was_read_is_refused() {
        let (_temp, mut host, project) = tio2();
        // Another session saves the rutile meanwhile.
        let path = project.join(format!("structures/{RUTILE}.json"));
        let changed = rutile_file(&project).replace("4.594", "4.6");
        std::fs::write(&path, &changed).unwrap();
        let refused = call_as(&mut host, "structure.set_cell", set_a(4.7)).unwrap_err();
        assert_eq!(refused.code, "project_changed");
        assert_eq!(rutile_file(&project), changed);
        // The lock went with the refusal.
        assert!(ProjectLock::acquire(&project, "x").is_ok());
    }

    #[test]
    fn imports_and_supercells_join_the_project_files() {
        let (temp, mut host, project) = tio2();
        let cif = temp.path().join("rutile.cif");
        std::fs::copy(fixtures().join("rutile.cif"), &cif).unwrap();
        let (_, manifest) = read_manifest(&project).unwrap();
        let input = json!({ "path": cif, "project": manifest.id.to_string() });
        let imported = call_as(&mut host, "file.import", input).unwrap();
        let new = imported["structures"][0]["structure"].clone();
        assert_eq!(imported["structures"][0]["name"], "rutile");
        let doubled = json!({
            "structure": 1,
            "matrix": [[1, 0, 0], [0, 1, 0], [0, 0, 2]],
            "name": "",
        });
        let made = call_as(&mut host, "structure.make_supercell", doubled).unwrap();
        assert_eq!(made["name"], "金红石 1×1×2 supercell");

        let (dir, manifest) = read_manifest(&project).unwrap();
        let names: Vec<&str> = manifest.entries.iter().map(|e| e.name.as_str()).collect();
        // The supercell after the rutile's earlier one, the import last.
        assert_eq!(
            names,
            [
                "金红石",
                "金红石 2×2×2 超胞",
                "金红石 1×1×2 supercell",
                "rutile"
            ]
        );
        let import = manifest.entries[3].provenance.import().unwrap();
        assert!(dir.read_source(import).is_ok(), "the file's copy is kept");
        assert!(dir.read_structure(manifest.entries[2].id).is_ok());
        // Edits to the imported structure are saved like any other.
        let edit = json!({ "structure": new, "values": [{ "parameter": "c", "value": 3.0 }] });
        call_as(&mut host, "structure.set_cell", edit).unwrap();
        let file = dir.read_structure(manifest.entries[3].id).unwrap();
        assert_eq!(file.structure.lattice.params().c.value(), 3.0);
    }

    #[test]
    fn files_are_written_where_allowed() {
        let (temp, mut host, project) = tio2();
        let confirmed = claude().confirmed();
        let registry = Registry::standard();
        let mut export = |path: PathBuf| {
            let input = json!({
                "structure": 1,
                "format": "cif",
                "path": path,
                "overwrite": false,
            });
            registry.call_as(&mut host, &confirmed, "structure.export_file", input)
        };
        assert!(export(temp.path().join("out.cif")).is_ok());
        assert!(export(project.join("out.cif")).is_ok());
        let outside = std::env::temp_dir().join("mw-mcp-outside.cif");
        assert_eq!(export(outside.clone()).unwrap_err().code, "not_allowed");
        assert!(!outside.exists());
    }
}
