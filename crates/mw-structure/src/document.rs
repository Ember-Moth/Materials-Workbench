use std::sync::Arc;

use gpui_kit::SharedString;
use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{CrystalDocument, Derived, Structure};
use mw_crystal_io::{Diagnostic, ImportedBlock, Severity};

use crate::diagnostics::DiagnosticRow;
use crate::format;

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

/// One open structure and everything shown about it. The structure, its
/// derived data, measurements and undo history are the domain document;
/// edits go through it (`CrystalDocument::execute`).
pub struct StructureDocument {
    pub(crate) project_name: SharedString,
    pub(crate) name: SharedString,
    pub(crate) crystal: CrystalDocument,
    pub(crate) diagnostics: Vec<DocumentDiagnostic>,
    /// The data block the structure was read from; a CIF export writes its
    /// kept fields back.
    pub(crate) source: Option<Arc<ImportedBlock>>,
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
            crystal: CrystalDocument::with_derived(sample.structure, sample.derived),
            diagnostics,
            source: None,
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

    /// The document for an imported data block, with the diagnostics of its
    /// import. `None` if the block yielded no structure.
    pub fn from_import(
        project_name: impl Into<SharedString>,
        name: impl Into<SharedString>,
        block: &Arc<ImportedBlock>,
    ) -> Option<Self> {
        let structure = block.structure.clone()?;
        Some(Self {
            project_name: project_name.into(),
            name: name.into(),
            crystal: CrystalDocument::new(structure),
            diagnostics: block
                .diagnostics
                .iter()
                .cloned()
                .map(DocumentDiagnostic::Import)
                .collect(),
            source: Some(block.clone()),
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
        let block = Arc::new(report.blocks[0].clone());
        let document = StructureDocument::from_import("p", "x", &block).expect("structure");
        assert_eq!(document.derived().atoms.len(), 8);
        // Three defaulted angles and one element taken from its label.
        assert_eq!(document.diagnostics.len(), 4);
        assert_eq!(document.space_group_symbol().as_deref(), Some("P1"));
        // A lone atom in a cubic cell: the atoms have the full cubic symmetry.
        let detected = document.derived().symmetry.detected.as_ref().unwrap();
        assert_eq!(detected.number, 221);
    }

    #[test]
    fn space_group_symbols_come_from_the_operations() {
        let rutile = StructureDocument::from_sample("p", "rutile", SampleKey::Rutile);
        assert_eq!(rutile.space_group_symbol().as_deref(), Some("P4₂/mnm"));
        let srtio3 = StructureDocument::from_sample("p", "SrTiO3", SampleKey::StrontiumTitanate);
        assert_eq!(srtio3.space_group_symbol().as_deref(), Some("Pm-3m"));
    }
}
