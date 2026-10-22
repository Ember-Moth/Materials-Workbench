use mw_crystal::sample::SampleKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProjectId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StructureEntryId(pub u32);

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

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct StructureEntry {
    pub id: StructureEntryId,
    pub name: String,
    pub origin: StructureOrigin,
    /// Mock: the built-in sample that stands in for the file.
    pub sample: SampleKey,
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
                            sample: SampleKey::Rutile,
                        },
                        StructureEntry {
                            id: StructureEntryId(2),
                            name: "金红石 2×2×2 超胞".into(),
                            origin: StructureOrigin::Derived { from: rutile },
                            sample: SampleKey::RutileSupercell,
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
                        sample: SampleKey::StrontiumTitanate,
                    }],
                },
            ],
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
        assert_eq!(entry.sample, SampleKey::StrontiumTitanate);
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
}
