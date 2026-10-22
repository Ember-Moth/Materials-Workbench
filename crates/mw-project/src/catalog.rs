use std::sync::Arc;

/// Capabilities name structures by this id, so it lives with them.
pub use mw_automation::StructureEntryId;
use mw_crystal::sample::SampleKey;
use mw_crystal_io::ImportedBlock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProjectId(pub u32);

/// Where a structure in a project came from.
#[derive(Clone, Debug, PartialEq)]
pub enum StructureOrigin {
    Imported {
        file_name: String,
    },
    /// Generated from another structure, e.g. a supercell.
    Derived {
        from: StructureEntryId,
    },
}

/// Where a structure's data is held.
#[derive(Clone, Debug, PartialEq)]
pub enum StructureSource {
    /// Mock: a built-in sample stands in for the structure a project file
    /// would hold (M6).
    Sample(SampleKey),
    /// A data block read from a file in this session, with the diagnostics
    /// of its import. Held in memory until project files exist (M6).
    Imported(Arc<ImportedBlock>),
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct StructureEntry {
    pub id: StructureEntryId,
    pub name: String,
    pub origin: StructureOrigin,
    pub source: StructureSource,
}

/// A structure to add to the catalog, as [`ProjectCatalog::add_imported`]
/// takes it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ImportedEntry {
    pub name: String,
    pub file_name: String,
    pub block: Arc<ImportedBlock>,
}

impl ImportedEntry {
    pub fn new(name: String, file_name: String, block: Arc<ImportedBlock>) -> Self {
        Self {
            name,
            file_name,
            block,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub structures: Vec<StructureEntry>,
}

/// All projects known to the application.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectCatalog {
    projects: Vec<Project>,
    /// The project that collects files opened outside any project.
    imports: Option<ProjectId>,
}

impl ProjectCatalog {
    /// Mock: two projects built from the samples, standing in for project
    /// files on disk.
    pub fn sample() -> Self {
        let rutile = StructureEntryId(1);
        Self {
            projects: vec![
                Project {
                    id: ProjectId(1),
                    name: "TiO₂ 相研究".into(),
                    structures: vec![
                        StructureEntry {
                            id: rutile,
                            name: "金红石".into(),
                            origin: StructureOrigin::Imported {
                                file_name: "rutile.cif".into(),
                            },
                            source: StructureSource::Sample(SampleKey::Rutile),
                        },
                        StructureEntry {
                            id: StructureEntryId(2),
                            name: "金红石 2×2×2 超胞".into(),
                            origin: StructureOrigin::Derived { from: rutile },
                            source: StructureSource::Sample(SampleKey::RutileSupercell),
                        },
                    ],
                },
                Project {
                    id: ProjectId(2),
                    name: "钙钛矿氧化物".into(),
                    structures: vec![StructureEntry {
                        id: StructureEntryId(3),
                        name: "SrTiO₃".into(),
                        origin: StructureOrigin::Imported {
                            file_name: "SrTiO3.cif".into(),
                        },
                        source: StructureSource::Sample(SampleKey::StrontiumTitanate),
                    }],
                },
            ],
            imports: None,
        }
    }

    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    /// The structure with this id and the project that contains it.
    pub fn structure(&self, id: StructureEntryId) -> Option<(&Project, &StructureEntry)> {
        self.projects.iter().find_map(|project| {
            project
                .structures
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| (project, entry))
        })
    }

    /// Adds imported structures to the project that collects opened files,
    /// creating it under `project_name` the first time. Returns the new
    /// entries' ids in order.
    ///
    /// Mock: until there are project files and a "new project" flow (M6),
    /// every opened file lands in this one in-memory project.
    pub fn add_imported(
        &mut self,
        project_name: &str,
        entries: impl IntoIterator<Item = ImportedEntry>,
    ) -> Vec<StructureEntryId> {
        let project_id = match self.imports {
            Some(id) => id,
            None => {
                let id = ProjectId(self.projects.iter().map(|p| p.id.0).max().unwrap_or(0) + 1);
                self.projects.push(Project {
                    id,
                    name: project_name.to_string(),
                    structures: Vec::new(),
                });
                self.imports = Some(id);
                id
            }
        };
        let mut next = self
            .projects
            .iter()
            .flat_map(|p| p.structures.iter().map(|s| s.id.0))
            .max()
            .unwrap_or(0);
        let Some(project) = self.projects.iter_mut().find(|p| p.id == project_id) else {
            return Vec::new();
        };
        entries
            .into_iter()
            .map(|entry| {
                next += 1;
                let id = StructureEntryId(next);
                project.structures.push(StructureEntry {
                    id,
                    name: entry.name,
                    origin: StructureOrigin::Imported {
                        file_name: entry.file_name,
                    },
                    source: StructureSource::Imported(entry.block),
                });
                id
            })
            .collect()
    }

    /// The first structure of the first project, if any.
    pub fn first_structure(&self) -> Option<StructureEntryId> {
        self.projects
            .iter()
            .find_map(|p| p.structures.first())
            .map(|s| s.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_structures_with_their_project() {
        let catalog = ProjectCatalog::sample();
        let (project, entry) = catalog.structure(StructureEntryId(3)).expect("SrTiO₃");
        assert_eq!(project.id, ProjectId(2));
        assert_eq!(
            entry.source,
            StructureSource::Sample(SampleKey::StrontiumTitanate)
        );
        assert!(catalog.structure(StructureEntryId(99)).is_none());
    }

    #[test]
    fn structure_ids_are_unique() {
        let catalog = ProjectCatalog::sample();
        let mut ids: Vec<_> = catalog
            .projects()
            .iter()
            .flat_map(|p| p.structures.iter().map(|s| s.id.0))
            .collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn imports_collect_in_one_new_project() {
        let mut catalog = ProjectCatalog::sample();
        let block = Arc::new(
            mw_crystal_io::cif::read_str("data_x\n_cell_length_a 1\n")
                .blocks
                .remove(0),
        );
        let entry = |name: &str| ImportedEntry::new(name.into(), "x.cif".into(), block.clone());
        let first = catalog.add_imported("Imported", [entry("a"), entry("b")]);
        let second = catalog.add_imported("ignored", [entry("c")]);
        assert_eq!(first, [StructureEntryId(4), StructureEntryId(5)]);
        assert_eq!(second, [StructureEntryId(6)]);
        assert_eq!(catalog.projects().len(), 3);
        let (project, entry) = catalog.structure(StructureEntryId(6)).expect("added");
        assert_eq!(project.name, "Imported");
        assert_eq!(project.structures.len(), 3);
        assert_eq!(
            entry.origin,
            StructureOrigin::Imported {
                file_name: "x.cif".into()
            }
        );
    }
}
