use std::sync::Arc;

use gpui_kit::SharedString;
use mw_crystal::{Checkpoint, Command, CrystalDocument, Derived, Structure};
use mw_crystal_io::{Diagnostic, ImportedBlock, Severity};
use mw_project_file::{Provenance, StructureFile, structure_sha256};

use crate::diagnostics::DiagnosticRow;
use crate::format;

/// A diagnostic shown in the diagnostics tab.
#[derive(Clone, Debug)]
pub(crate) enum DocumentDiagnostic {
    Import(Diagnostic),
    /// The project's copy of the imported file is missing or changed, so
    /// its diagnostics and the fields the structure does not hold are gone
    /// (`docs/design/project-format.md` §7.1).
    SourceUnavailable {
        file_name: String,
    },
}

impl DocumentDiagnostic {
    pub fn row(&self) -> DiagnosticRow {
        match self {
            Self::Import(diagnostic) => DiagnosticRow::from_import(diagnostic),
            Self::SourceUnavailable { file_name } => DiagnosticRow::new(
                Severity::Warning,
                tr!("diagnostic.source_unavailable", file = file_name.clone()),
                None,
            ),
        }
    }
}

/// One open structure and everything shown about it. The structure, its
/// derived data, measurements and undo history are the domain document;
/// edits go through it (`CrystalDocument::execute`).
pub struct StructureDocument {
    pub(crate) project_name: SharedString,
    pub(crate) name: SharedString,
    pub(crate) crystal: CrystalDocument,
    pub(crate) diagnostics: Vec<DocumentDiagnostic>,
    /// Where the structure came from, as the project records it.
    pub(crate) provenance: Provenance,
    /// For an imported structure, the data block read again from the
    /// project's copy of the file: a CIF export writes its kept fields back.
    pub(crate) source: Option<Arc<ImportedBlock>>,
    /// The commands applied in earlier sessions, as read; this session's
    /// follow them when saving (`docs/design/project-format.md` §7.3).
    edits: Vec<Command>,
}

impl StructureDocument {
    /// The document for a structure of a project. `source` is the data
    /// block an imported structure came from, when its copy could be read;
    /// its diagnostics are shown.
    pub fn new(
        project_name: impl Into<SharedString>,
        name: impl Into<SharedString>,
        file: StructureFile,
        provenance: Provenance,
        source: Option<Arc<ImportedBlock>>,
    ) -> Self {
        let diagnostics = match (provenance.import(), &source) {
            (_, Some(block)) => block
                .diagnostics
                .iter()
                .cloned()
                .map(DocumentDiagnostic::Import)
                .collect(),
            (Some(import), None) => vec![DocumentDiagnostic::SourceUnavailable {
                file_name: import.file_name.clone(),
            }],
            (None, None) => Vec::new(),
        };
        Self {
            project_name: project_name.into(),
            name: name.into(),
            crystal: CrystalDocument::restore(file.structure, file.measurements),
            diagnostics,
            provenance,
            source,
            edits: file.edits,
        }
    }

    pub(crate) fn structure(&self) -> &Structure {
        self.crystal.structure()
    }

    pub(crate) fn derived(&self) -> &Derived {
        self.crystal.derived()
    }

    /// The identified space group type in display form, e.g. `P4₂/mnm`.
    pub(crate) fn space_group_symbol(&self) -> Option<String> {
        let space_group = self.derived().symmetry.space_group.as_ref()?;
        Some(format::space_group(space_group.hm_symbol))
    }

    /// Whether the structure differs from the one imported, which decides
    /// whether a CIF export writes back the fields it did not recognize.
    pub(crate) fn changed_since_import(&self) -> bool {
        self.provenance
            .import()
            .is_none_or(|import| structure_sha256(self.structure()) != import.structure_sha256)
    }

    /// What the project file holds for this document: the structure, its
    /// measurements and the record of every command applied to it.
    pub fn file(&self) -> StructureFile {
        let edits = self
            .edits
            .iter()
            .chain(self.crystal.history())
            .cloned()
            .collect();
        StructureFile::new(
            self.structure().clone(),
            self.crystal.annotations().to_vec(),
            edits,
        )
    }

    /// Whether there are edits since the document was read or saved.
    pub fn is_modified(&self) -> bool {
        self.crystal.is_modified()
    }

    pub fn checkpoint(&self) -> Checkpoint {
        self.crystal.checkpoint()
    }

    pub(crate) fn mark_saved(&mut self, checkpoint: Checkpoint) {
        self.crystal.mark_saved(checkpoint);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mw_crystal::sample::{self, SampleKey};
    use mw_crystal::{CellParam, CellValues, Edit};
    use mw_project_file::{Created, Import, Sha256, SourceFormat, Timestamp};

    /// A document for a built-in sample, made from nothing in the
    /// application.
    pub(crate) fn sample_document(key: SampleKey) -> StructureDocument {
        document_of(sample::load(key).structure)
    }

    pub(crate) fn document_of(structure: mw_crystal::Structure) -> StructureDocument {
        StructureDocument::new(
            "Samples",
            "Test",
            StructureFile::new(structure, Vec::new(), Vec::new()),
            Provenance::Created(Created::new(Timestamp::from_unix(0))),
            None,
        )
    }

    fn imported(text: &str) -> (Arc<ImportedBlock>, Provenance) {
        let report = mw_crystal_io::cif::read_str(text);
        let block = Arc::new(report.blocks[0].clone());
        let import = Import::new(
            SourceFormat::Cif,
            "x.cif",
            None,
            Sha256::of(text.as_bytes()),
            "x",
            Timestamp::from_unix(0),
            structure_sha256(block.structure.as_ref().unwrap()),
        );
        (block, Provenance::Import(import))
    }

    const NA: &str = "data_x\n_space_group_name_H-M_alt 'P 1'\n\
        _cell_length_a 4\n_cell_length_b 4\n_cell_length_c 4\n\
        loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
        Na1 0 0 0\n";

    #[test]
    fn imported_documents_keep_the_import_diagnostics() {
        let (block, provenance) = imported(NA);
        let file = StructureFile::new(block.structure.clone().unwrap(), Vec::new(), Vec::new());
        let document =
            StructureDocument::new("p", "x", file.clone(), provenance.clone(), Some(block));
        let one_cell = document
            .derived()
            .display(document.structure(), &Default::default());
        assert_eq!(one_cell.atoms.len(), 8);
        // Three defaulted angles and one element taken from its label.
        assert_eq!(document.diagnostics.len(), 4);
        assert_eq!(document.space_group_symbol().as_deref(), Some("P1"));
        // A lone atom in a cubic cell: the atoms have the full cubic symmetry.
        let detected = document.derived().symmetry.detected.as_ref().unwrap();
        assert_eq!(detected.number, 221);
        assert!(!document.changed_since_import());

        // Without the file's copy, the diagnostics say so.
        let document = StructureDocument::new("p", "x", file, provenance, None);
        assert!(matches!(
            &document.diagnostics[..],
            [DocumentDiagnostic::SourceUnavailable { file_name }] if file_name == "x.cif"
        ));
    }

    #[test]
    fn the_file_holds_every_command_since_the_import() {
        let (block, provenance) = imported(NA);
        let earlier = Command::Edit(Edit::SetCell {
            values: CellValues::default().with(CellParam::A, 4.5),
        });
        let mut structure = block.structure.clone().unwrap();
        let mut params = *structure.lattice.params();
        params.a = 4.5.into();
        structure.lattice.set_params(params);
        let file = StructureFile::new(structure, Vec::new(), vec![earlier.clone()]);
        let mut document = StructureDocument::new("p", "x", file, provenance, Some(block));
        assert!(document.changed_since_import());
        assert!(!document.is_modified());
        let now = Edit::SetCell {
            values: CellValues::default().with(CellParam::A, 5.0),
        };
        document.crystal.edit(now.clone()).unwrap();
        assert!(document.is_modified());
        let saved = document.file();
        assert_eq!(saved.edits, [earlier, Command::Edit(now)]);
        assert_eq!(saved.structure.lattice.params().a.value(), 5.0);
        let checkpoint = document.checkpoint();
        document.mark_saved(checkpoint);
        assert!(!document.is_modified());
    }

    #[test]
    fn space_group_symbols_come_from_the_operations() {
        let rutile = sample_document(SampleKey::Rutile);
        assert_eq!(rutile.space_group_symbol().as_deref(), Some("P4₂/mnm"));
        let srtio3 = sample_document(SampleKey::StrontiumTitanate);
        assert_eq!(srtio3.space_group_symbol().as_deref(), Some("Pm-3m"));
    }
}
