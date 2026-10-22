use gpui_kit::SharedString;
use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{Derived, Measurement, Structure};
use mw_crystal_io::{Diagnostic, ImportedBlock, Severity};

use crate::diagnostics::DiagnosticRow;

/// A diagnostic shown in the diagnostics tab.
#[derive(Clone, Debug)]
pub(crate) enum DocumentDiagnostic {
    Import(Diagnostic),
    /// Mock: stands in for the provenance note a derived structure will
    /// carry (M5).
    SupercellInP1,
}

impl DocumentDiagnostic {
    pub fn row(&self) -> DiagnosticRow {
        match self {
            Self::Import(diagnostic) => DiagnosticRow::from_import(diagnostic),
            Self::SupercellInP1 => {
                DiagnosticRow::new(Severity::Warning, tr!("mock.diagnostic.supercell"), None)
            }
        }
    }
}

/// One open structure and everything shown about it.
pub struct StructureDocument {
    pub(crate) project_name: SharedString,
    pub(crate) name: SharedString,
    pub(crate) structure: Structure,
    pub(crate) derived: Derived,
    /// Distances and angles measured in this session, in the order taken.
    pub(crate) measurements: Vec<Measurement>,
    pub(crate) diagnostics: Vec<DocumentDiagnostic>,
}

impl StructureDocument {
    /// Mock: builds the document from a built-in sample instead of a project
    /// file.
    pub fn from_sample(
        project_name: impl Into<SharedString>,
        name: impl Into<SharedString>,
        key: SampleKey,
    ) -> Self {
        let sample = sample::load(key);
        let diagnostics = match key {
            SampleKey::RutileSupercell => vec![DocumentDiagnostic::SupercellInP1],
            SampleKey::Rutile | SampleKey::StrontiumTitanate => Vec::new(),
        };
        Self {
            project_name: project_name.into(),
            name: name.into(),
            structure: sample.structure,
            derived: sample.derived,
            measurements: Vec::new(),
            diagnostics,
        }
    }

    /// The document for an imported data block, with the diagnostics of its
    /// import. `None` if the block yielded no structure.
    pub fn from_import(
        project_name: impl Into<SharedString>,
        name: impl Into<SharedString>,
        block: &ImportedBlock,
    ) -> Option<Self> {
        let structure = block.structure.clone()?;
        Some(Self {
            project_name: project_name.into(),
            name: name.into(),
            derived: Derived::compute(&structure),
            structure,
            measurements: Vec::new(),
            diagnostics: block
                .diagnostics
                .iter()
                .cloned()
                .map(DocumentDiagnostic::Import)
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_documents_keep_the_import_diagnostics() {
        let report = mw_crystal_io::cif::read_str(
            "data_x\n_space_group_name_H-M_alt 'P 1'\n\
             _cell_length_a 4\n_cell_length_b 4\n_cell_length_c 4\n\
             loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
             Na1 0 0 0\n",
        );
        let document =
            StructureDocument::from_import("p", "x", &report.blocks[0]).expect("structure");
        assert_eq!(document.derived.atoms.len(), 8);
        // Three defaulted angles and one element taken from its label.
        assert_eq!(document.diagnostics.len(), 4);
        assert!(document.derived.symmetry.is_none());
    }
}
