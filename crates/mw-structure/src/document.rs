use gpui_kit::SharedString;
use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{Derived, Lattice, Structure};
use mw_crystal_io::{Diagnostic, ImportedBlock, Severity};

use crate::diagnostics::DiagnosticRow;
use crate::format;

/// A distance between two atoms, as shown in the measurements panel and as
/// a label in the viewport.
#[derive(Clone, Debug)]
pub(crate) struct Measurement {
    pub label: SharedString,
    /// Cartesian Å.
    pub from: [f64; 3],
    pub to: [f64; 3],
}

impl Measurement {
    fn between(lattice: &Lattice, label: &str, from: [f64; 3], to: [f64; 3]) -> Self {
        Self {
            label: label.to_string().into(),
            from: lattice.to_cartesian(from),
            to: lattice.to_cartesian(to),
        }
    }

    pub fn distance(&self) -> f64 {
        self.from
            .iter()
            .zip(self.to)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            .sqrt()
    }

    pub fn midpoint(&self) -> [f64; 3] {
        [0, 1, 2].map(|i| (self.from[i] + self.to[i]) / 2.0)
    }
}

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
    pub(crate) measurements: Vec<Measurement>,
    pub(crate) diagnostics: Vec<DocumentDiagnostic>,
}

impl StructureDocument {
    /// Mock: builds the document from a built-in sample instead of a project
    /// file, with hand-written measurements.
    pub fn from_sample(
        project_name: impl Into<SharedString>,
        name: impl Into<SharedString>,
        key: SampleKey,
    ) -> Self {
        let sample = sample::load(key);
        let lattice = sample.structure.lattice;
        let (measurements, diagnostics) = match key {
            SampleKey::Rutile => (
                vec![Measurement::between(
                    &lattice,
                    "Ti1–O1",
                    [0.5, 0.5, 0.5],
                    [0.8048, 0.1952, 0.5],
                )],
                Vec::new(),
            ),
            SampleKey::StrontiumTitanate => (
                vec![Measurement::between(
                    &lattice,
                    "Ti1–O1",
                    [0.5, 0.5, 0.5],
                    [0.5, 0.5, 0.0],
                )],
                Vec::new(),
            ),
            SampleKey::RutileSupercell => (Vec::new(), vec![DocumentDiagnostic::SupercellInP1]),
        };
        Self {
            project_name: project_name.into(),
            name: name.into(),
            structure: sample.structure,
            derived: sample.derived,
            measurements,
            diagnostics,
        }
    }

    /// The identified space group type in display form, e.g. `P4₂/mnm`.
    pub(crate) fn space_group_symbol(&self) -> Option<String> {
        let space_group = self.derived.symmetry.space_group.as_ref()?;
        Some(format::space_group(space_group.hm_symbol))
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
    fn rutile_apical_bond_length() {
        let document = StructureDocument::from_sample("p", "rutile", SampleKey::Rutile);
        let distance = document.measurements[0].distance();
        assert!((distance - 1.980).abs() < 1e-3, "{distance}");
    }

    #[test]
    fn strontium_titanate_ti_o_is_half_the_cell() {
        let document = StructureDocument::from_sample("p", "SrTiO3", SampleKey::StrontiumTitanate);
        assert!((document.measurements[0].distance() - 3.905 / 2.0).abs() < 1e-9);
    }

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
        assert_eq!(document.space_group_symbol().as_deref(), Some("P1"));
        // A lone atom in a cubic cell: the atoms have the full cubic symmetry.
        let detected = document.derived.symmetry.detected.as_ref().unwrap();
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
