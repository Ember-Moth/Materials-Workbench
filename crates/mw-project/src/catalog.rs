use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Capabilities name structures by this id, so it lives with them.
pub use mw_automation::StructureEntryId;
use mw_crystal::Structure;
use mw_crystal_io::ImportedBlock;
use mw_project_file::{
    Derived, Entry, EntryId, EntryKind, Import, Manifest, Operation, ProjectDir, Provenance, Save,
    Sha256, Source, SourceFormat, StructureFile, Timestamp, ViewState, structure_sha256,
};

/// A project open in this session. Files name projects by their own ids;
/// this one is for the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProjectId(pub u32);

/// A structure whose content is not in the project directory yet: imported
/// or derived since the project was last saved.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Pending {
    pub file: Arc<StructureFile>,
    /// For an imported structure, the data block it came from.
    pub block: Option<Arc<ImportedBlock>>,
}

/// One structure of a project.
#[derive(Clone, Debug)]
pub struct StructureEntry {
    id: StructureEntryId,
    entry: Entry,
    pending: Option<Pending>,
}

impl StructureEntry {
    pub fn id(&self) -> StructureEntryId {
        self.id
    }

    /// The entry as the project file lists it: its file id, name and
    /// provenance.
    pub fn entry(&self) -> &Entry {
        &self.entry
    }

    pub fn name(&self) -> &str {
        &self.entry.name
    }

    pub fn is_derived(&self) -> bool {
        matches!(self.entry.provenance, Provenance::Derived(_))
    }

    /// The content when it is not on disk yet.
    pub fn pending(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }
}

/// A project: on disk once saved, or untitled until then.
#[derive(Clone, Debug)]
pub struct Project {
    id: ProjectId,
    manifest: Manifest,
    structures: Vec<StructureEntry>,
    dir: Option<ProjectDir>,
    /// Copies of imported files not in the directory yet.
    sources: Vec<Source>,
    view: ViewState,
    /// Counts changes to the project itself (entries added, renamed), so
    /// that a save finishing after another change leaves it modified.
    revision: u64,
    saved_revision: u64,
}

impl Project {
    pub fn id(&self) -> ProjectId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub fn structures(&self) -> &[StructureEntry] {
        &self.structures
    }

    pub fn structure(&self, id: StructureEntryId) -> Option<&StructureEntry> {
        self.structures.iter().find(|s| s.id == id)
    }

    /// The directory, once the project is saved.
    pub fn dir(&self) -> Option<&ProjectDir> {
        self.dir.as_ref()
    }

    pub fn path(&self) -> Option<&Path> {
        self.dir.as_ref().map(ProjectDir::path)
    }

    /// The project's id in its files.
    pub fn file_id(&self) -> mw_project_file::ProjectId {
        self.manifest.id
    }

    /// Whether the project itself has unsaved changes: structures added
    /// (an untitled project with structures has them all unsaved), or an
    /// older format read. Edits to open structures are tracked by their
    /// documents.
    pub fn is_modified(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn is_untitled(&self) -> bool {
        self.dir.is_none()
    }

    /// The view state as last read or saved.
    pub fn view(&self) -> &ViewState {
        &self.view
    }

    /// What to write to save the project: `structures` (the open documents
    /// that changed) and everything that is not on disk yet.
    pub fn save_request(&self, structures: Vec<(EntryId, StructureFile)>, view: ViewState) -> Save {
        let mut structures = structures;
        for entry in &self.structures {
            if let Some(pending) = &entry.pending
                && !structures.iter().any(|(id, _)| *id == entry.entry.id)
            {
                structures.push((entry.entry.id, StructureFile::clone(&pending.file)));
            }
        }
        Save {
            manifest: self.manifest.clone(),
            structures,
            sources: self.sources.clone(),
            from: self.dir.clone(),
            view: Some(view),
        }
    }
}

/// A structure read from a file, to add to a project.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ImportedEntry {
    pub name: String,
    pub path: PathBuf,
    pub format: SourceFormat,
    /// The whole file, for the project's copy.
    pub bytes: Arc<[u8]>,
    pub block: Arc<ImportedBlock>,
}

impl ImportedEntry {
    pub fn new(
        name: String,
        path: PathBuf,
        format: SourceFormat,
        bytes: Arc<[u8]>,
        block: Arc<ImportedBlock>,
    ) -> Self {
        Self {
            name,
            path,
            format,
            bytes,
            block,
        }
    }
}

/// A structure made from another one, to add to a project.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DerivedEntry {
    pub name: String,
    pub structure: Structure,
    pub operation: Operation,
}

impl DerivedEntry {
    pub fn new(name: String, structure: Structure, operation: Operation) -> Self {
        Self {
            name,
            structure,
            operation,
        }
    }
}

/// The projects open in this session.
#[derive(Clone, Debug, Default)]
pub struct ProjectCatalog {
    projects: Vec<Project>,
    next_project: u32,
    next_structure: u32,
}

impl ProjectCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    fn project_mut(&mut self, id: ProjectId) -> Option<&mut Project> {
        self.projects.iter_mut().find(|p| p.id == id)
    }

    /// The structure with this id and the project that contains it.
    pub fn structure(&self, id: StructureEntryId) -> Option<(&Project, &StructureEntry)> {
        self.projects
            .iter()
            .find_map(|project| project.structure(id).map(|entry| (project, entry)))
    }

    /// The open project read from this directory, or with this file id
    /// (the same project moved or copied while open).
    pub fn find(&self, path: &Path, file_id: mw_project_file::ProjectId) -> Option<ProjectId> {
        self.projects
            .iter()
            .find(|p| p.path() == Some(path) || p.file_id() == file_id)
            .map(|p| p.id)
    }

    fn next_project_id(&mut self) -> ProjectId {
        self.next_project += 1;
        ProjectId(self.next_project)
    }

    fn next_structure_id(&mut self) -> StructureEntryId {
        self.next_structure += 1;
        StructureEntryId(self.next_structure)
    }

    fn push(&mut self, manifest: Manifest, dir: Option<ProjectDir>, view: ViewState) -> ProjectId {
        let id = self.next_project_id();
        let structures = manifest
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::CrystalStructure)
            .cloned()
            .map(|entry| StructureEntry {
                id: self.next_structure_id(),
                entry,
                pending: None,
            })
            .collect();
        // An older format is migrated in memory and saved in the new one.
        let revision = u64::from(dir.as_ref().is_some_and(ProjectDir::is_migrated));
        self.projects.push(Project {
            id,
            manifest,
            structures,
            dir,
            sources: Vec::new(),
            view,
            revision,
            saved_revision: 0,
        });
        id
    }

    /// Adds a project read from disk.
    pub fn open(&mut self, dir: ProjectDir, manifest: Manifest, view: ViewState) -> ProjectId {
        self.push(manifest, Some(dir), view)
    }

    /// Adds an empty, untitled project.
    pub fn create(&mut self, name: impl Into<String>) -> ProjectId {
        self.push(Manifest::new(name), None, ViewState::default())
    }

    pub fn close(&mut self, id: ProjectId) -> Option<Project> {
        let index = self.projects.iter().position(|p| p.id == id)?;
        Some(self.projects.remove(index))
    }

    /// Adds imported structures to a project, recording their provenance
    /// and keeping a copy of each file. Returns the new entries' ids.
    pub fn add_imported(
        &mut self,
        project: ProjectId,
        entries: impl IntoIterator<Item = ImportedEntry>,
    ) -> Vec<StructureEntryId> {
        let now = Timestamp::now();
        let mut added = Vec::new();
        for imported in entries {
            let Some(structure) = imported.block.structure.clone() else {
                continue;
            };
            let id = self.next_structure_id();
            let Some(project) = self.project_mut(project) else {
                return added;
            };
            let sha = Sha256::of(&imported.bytes);
            if !project.sources.iter().any(|s| s.sha256 == sha) {
                project.sources.push(Source {
                    sha256: sha.clone(),
                    format: imported.format,
                    bytes: imported.bytes.clone(),
                });
            }
            let file_name = imported
                .path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let import = Import::new(
                imported.format,
                file_name,
                Some(imported.path.display().to_string()),
                sha,
                imported.block.name.clone(),
                now.clone(),
                structure_sha256(&structure),
            );
            let entry = Entry::new(
                EntryId::new(),
                EntryKind::CrystalStructure,
                imported.name,
                Provenance::Import(import),
            );
            project.add(StructureEntry {
                id,
                entry,
                pending: Some(Pending {
                    file: Arc::new(StructureFile::new(structure, Vec::new(), Vec::new())),
                    block: Some(imported.block),
                }),
            });
            added.push(id);
        }
        added
    }

    /// Adds a structure made from `from` to its project, right after the
    /// structures already made from it, recording the operation. `from`
    /// is the source structure as it is now.
    pub fn add_derived(
        &mut self,
        from: StructureEntryId,
        source: &Structure,
        derived: DerivedEntry,
    ) -> Option<StructureEntryId> {
        let id = self.next_structure_id();
        let project = self
            .projects
            .iter_mut()
            .find(|p| p.structure(from).is_some())?;
        let index = project.structures.iter().position(|s| s.id == from)?;
        let from_entry = project.structures[index].entry.clone();
        let record = Derived::new(
            from_entry.id,
            from_entry.name.clone(),
            structure_sha256(source),
            derived.operation,
            Timestamp::now(),
        );
        let entry = Entry::new(
            EntryId::new(),
            EntryKind::CrystalStructure,
            derived.name,
            Provenance::Derived(record),
        );
        let after = project.structures[index + 1..]
            .iter()
            .take_while(|s| {
                matches!(&s.entry.provenance, Provenance::Derived(d) if d.from == from_entry.id)
            })
            .count();
        let position = index + 1 + after;
        let structure_entry = StructureEntry {
            id,
            entry,
            pending: Some(Pending {
                file: Arc::new(StructureFile::new(
                    derived.structure,
                    Vec::new(),
                    Vec::new(),
                )),
                block: None,
            }),
        };
        project
            .manifest
            .entries
            .insert(position, structure_entry.entry.clone());
        project.structures.insert(position, structure_entry);
        project.revision += 1;
        Some(id)
    }

    /// Records a finished save of `project` to `dir`: the entries written
    /// are on disk now, the project has the name it was saved under, and it
    /// is unmodified unless it changed after `snapshot` was taken.
    pub fn saved(&mut self, project: ProjectId, dir: ProjectDir, snapshot: &SaveSnapshot) {
        let Some(project) = self.project_mut(project) else {
            return;
        };
        for entry in &mut project.structures {
            if snapshot.entries.contains(&entry.entry.id) {
                entry.pending = None;
            }
        }
        project
            .sources
            .retain(|source| !snapshot.sources.contains(&source.sha256));
        // Saving under a new name renames the project; that is part of the
        // save, not a change after it.
        project.manifest.name = snapshot.name.clone();
        if project.revision == snapshot.revision {
            project.saved_revision = project.revision;
        }
        project.dir = Some(dir);
        project.view = snapshot.view.clone();
    }

    /// Takes the facts [`Self::saved`] needs, when a save starts.
    pub fn snapshot(&self, project: ProjectId, save: &Save) -> Option<SaveSnapshot> {
        let project = self.project(project)?;
        Some(SaveSnapshot {
            name: save.manifest.name.clone(),
            revision: project.revision,
            entries: save.manifest.entries.iter().map(|e| e.id).collect(),
            sources: save.sources.iter().map(|s| s.sha256.clone()).collect(),
            view: save.view.clone().unwrap_or_default(),
        })
    }

    /// The first structure of the first project, if any.
    pub fn first_structure(&self) -> Option<StructureEntryId> {
        self.projects
            .iter()
            .find_map(|p| p.structures.first())
            .map(|s| s.id)
    }
}

impl Project {
    fn add(&mut self, entry: StructureEntry) {
        self.manifest.entries.push(entry.entry.clone());
        self.structures.push(entry);
        self.revision += 1;
    }
}

/// What a save in progress wrote, for [`ProjectCatalog::saved`].
#[derive(Clone, Debug)]
pub struct SaveSnapshot {
    name: String,
    revision: u64,
    entries: Vec<EntryId>,
    sources: Vec<Sha256>,
    view: ViewState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use mw_crystal::sample::{self, SampleKey};

    fn imported(name: &str, text: &str) -> ImportedEntry {
        let bytes: Arc<[u8]> = text.as_bytes().into();
        let block = mw_crystal_io::cif::read(&bytes).blocks.remove(0);
        ImportedEntry::new(
            name.into(),
            PathBuf::from(format!("/data/{name}.cif")),
            SourceFormat::Cif,
            bytes,
            Arc::new(block),
        )
    }

    const CIF: &str = "data_x\n_space_group_name_H-M_alt 'P 1'\n\
        _cell_length_a 4\n_cell_length_b 4\n_cell_length_c 4\n\
        loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
        Na1 0 0 0\n";

    #[test]
    fn imports_record_their_provenance_and_keep_the_file() {
        let mut catalog = ProjectCatalog::new();
        let project = catalog.create("未命名项目");
        let ids = catalog.add_imported(project, [imported("a", CIF), imported("b", CIF)]);
        assert_eq!(ids, [StructureEntryId(1), StructureEntryId(2)]);
        let (found, entry) = catalog.structure(ids[1]).unwrap();
        assert_eq!(found.id(), project);
        assert!(found.is_modified());
        let import = entry.entry().provenance.import().unwrap();
        assert_eq!(import.file_name, "b.cif");
        assert_eq!(import.path.as_deref(), Some("/data/b.cif"));
        assert_eq!(import.sha256, Sha256::of(CIF.as_bytes()));
        // One copy for both: they are the same file.
        let save = found.save_request(Vec::new(), ViewState::default());
        assert_eq!(save.sources.len(), 1);
        assert_eq!(save.structures.len(), 2);
        assert_eq!(save.manifest.entries.len(), 2);
        assert!(save.from.is_none());
    }

    #[test]
    fn derived_structures_follow_their_source() {
        let mut catalog = ProjectCatalog::new();
        let project = catalog.create("p");
        let ids = catalog.add_imported(project, [imported("a", CIF), imported("b", CIF)]);
        let source = sample::load(SampleKey::Rutile).structure;
        let derive = |name: &str| {
            DerivedEntry::new(
                name.into(),
                source.clone(),
                Operation::MakeSupercell {
                    matrix: [[2, 0, 0], [0, 2, 0], [0, 0, 2]],
                },
            )
        };
        let first = catalog.add_derived(ids[0], &source, derive("a 2")).unwrap();
        let second = catalog.add_derived(ids[0], &source, derive("a 3")).unwrap();
        let project = catalog.project(project).unwrap();
        let names: Vec<_> = project.structures().iter().map(|s| s.name()).collect();
        assert_eq!(names, ["a", "a 2", "a 3", "b"]);
        let entry = project.structure(second).unwrap();
        assert!(entry.is_derived() && entry.pending().is_some());
        let Provenance::Derived(record) = &entry.entry().provenance else {
            panic!("derived");
        };
        assert_eq!(record.from_name, "a");
        assert_eq!(record.from_structure_sha256, structure_sha256(&source));
        assert_ne!(first, second);
        let manifest: Vec<_> = project
            .save_request(Vec::new(), ViewState::default())
            .manifest
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(manifest, names);
    }

    #[test]
    fn saving_clears_what_was_written_only() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("p.mwproj");
        let mut catalog = ProjectCatalog::new();
        let project = catalog.create("p");
        catalog.add_imported(project, [imported("a", CIF)]);
        let save = catalog
            .project(project)
            .unwrap()
            .save_request(Vec::new(), ViewState::default());
        let snapshot = catalog.snapshot(project, &save).unwrap();
        let dir = mw_project_file::save(&path, &save).unwrap();
        // Something was imported while the save was writing.
        let later = catalog.add_imported(project, [imported("b", CIF)]);
        catalog.saved(project, dir.clone(), &snapshot);
        let found = catalog.project(project).unwrap();
        assert_eq!(found.path(), Some(path.as_path()));
        assert!(found.structures()[0].pending().is_none());
        assert!(found.structure(later[0]).unwrap().pending().is_some());
        assert!(found.is_modified());

        let save = found.save_request(Vec::new(), ViewState::default());
        let snapshot = catalog.snapshot(project, &save).unwrap();
        let dir = mw_project_file::save(&path, &save).unwrap();
        catalog.saved(project, dir, &snapshot);
        assert!(!catalog.project(project).unwrap().is_modified());

        // Opened again, the project has the same structures.
        let (dir, manifest) = mw_project_file::read_manifest(&path).unwrap();
        let mut reopened = ProjectCatalog::new();
        let id = reopened.open(dir, manifest, ViewState::default());
        assert_eq!(
            reopened.find(&path, catalog.project(project).unwrap().file_id()),
            Some(id)
        );
        let names: Vec<_> = reopened
            .project(id)
            .unwrap()
            .structures()
            .iter()
            .map(|s| s.name().to_string())
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert!(!reopened.project(id).unwrap().is_modified());
    }
}
