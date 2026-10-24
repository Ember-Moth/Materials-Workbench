//! Reading and writing crystal structure files, independent of any UI.
//!
//! [`cif::read`] imports the CIF 1.1 subset described in §9.1 of the design
//! notes in the Materials Workbench repository,
//! [`docs/design/crystallography-model.md`][design] (in Chinese; "design
//! §n" below refers to their sections): one [`ImportedBlock`] per
//! data block, each with its structure (unless it has errors) and the
//! [`Diagnostic`]s found in it, every one tied to a source line. Structures
//! come out normalized: sites snapped onto special positions, mixed
//! occupancy merged into one site, repeated rows dropped. The block also
//! keeps the fields the structure does not hold ([`CifFields`]).
//!
//! [`poscar::read`] imports a VASP POSCAR or CONTCAR file (design §9.3) as
//! one block holding a P1 structure, whose lattice keeps the orientation of
//! the file's vectors.
//!
//! [`cif::write`], [`poscar::write`] and [`xyz::write`] export a structure
//! as text (design §9.2–9.4). POSCAR and XYZ list atoms one by one, so they
//! refuse structures with partially occupied sites ([`ExportError`]).
//!
//! The structures are those of the `mw-crystal` crate.
//!
//! # Example
//!
//! ```
//! use mw_crystal_io::{cif, poscar};
//!
//! let text = "\
//! data_rutile
//! _space_group_name_H-M_alt 'P 42/m n m'
//! _cell_length_a 4.594
//! _cell_length_b 4.594
//! _cell_length_c 2.959
//! _cell_angle_alpha 90
//! _cell_angle_beta 90
//! _cell_angle_gamma 90
//! loop_
//! _atom_site_label
//! _atom_site_type_symbol
//! _atom_site_fract_x
//! _atom_site_fract_y
//! _atom_site_fract_z
//! Ti1 Ti 0 0 0
//! O1 O 0.3048 0.3048 0
//! ";
//! let report = cif::read_str(text);
//! for diagnostic in report.all_diagnostics() {
//!     println!("line {:?}: {:?}", diagnostic.line(), diagnostic.kind());
//! }
//! let (block, rutile) = report.structures().next().unwrap();
//! assert_eq!(block.name, "rutile");
//! assert_eq!(rutile.symmetry.operations().len(), 16);
//!
//! // POSCAR lists the 6 atoms of the cell, grouped by element.
//! let vasp = poscar::write(rutile, "rutile").unwrap();
//! let back = poscar::read_str(&vasp);
//! let (_, p1) = back.structures().next().unwrap();
//! assert_eq!(p1.sites.len(), 6);
//! ```
//!
//! [design]: https://github.com/Ember-Moth/Materials-Workbench/blob/main/docs/design/crystallography-model.md

// The README's example runs with the doc tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

mod atoms;
mod cif_write;
mod diagnostic;
mod import;
mod kept;
mod normalize;
mod number;
pub mod poscar;
mod poscar_read;
mod syntax;
pub mod xyz;

pub use atoms::ExportError;
pub use diagnostic::{Diagnostic, DiagnosticKind, Severity};
pub use kept::CifFields;

use mw_crystal::Structure;

/// The result of reading one file.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ImportReport {
    /// One entry per data block, in file order.
    pub blocks: Vec<ImportedBlock>,
    /// Problems that belong to no block, such as text before the first one.
    pub diagnostics: Vec<Diagnostic>,
}

/// One data block of a file.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ImportedBlock {
    /// The block code, e.g. `rutile` for `data_rutile`.
    pub name: String,
    /// Line of the `data_` header.
    pub line: u32,
    /// `None` when the block has errors or holds no structure.
    pub structure: Option<Structure>,
    /// Sorted by line.
    pub diagnostics: Vec<Diagnostic>,
    /// The fields the structure does not hold, for writing the block back.
    pub fields: CifFields,
}

impl ImportReport {
    /// The blocks that yielded a structure.
    pub fn structures(&self) -> impl Iterator<Item = (&ImportedBlock, &Structure)> {
        self.blocks
            .iter()
            .filter_map(|block| Some((block, block.structure.as_ref()?)))
    }

    /// Every diagnostic: the file's own, then each block's.
    pub fn all_diagnostics(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .chain(self.blocks.iter().flat_map(|b| &b.diagnostics))
    }

    /// Blocks that describe a structure the importer could not build.
    pub fn failed_blocks(&self) -> impl Iterator<Item = &ImportedBlock> {
        self.blocks.iter().filter(|block| block.has_errors())
    }
}

impl ImportedBlock {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity() == Severity::Error)
    }

    /// Options for writing `structure`, read from this block and possibly
    /// edited since, back to CIF: the block's name and its kept fields,
    /// the unrecognized ones only while the structure is unchanged.
    pub fn write_options(&self, structure: &Structure) -> cif::WriteOptions<'_> {
        let changed = self.structure.as_ref() != Some(structure);
        cif::WriteOptions::new(&self.name).kept(&self.fields, changed)
    }
}

pub mod cif {
    //! CIF 1.1 import and export.

    use std::borrow::Cow;

    use mw_crystal::Tolerances;

    pub use crate::cif_write::{WriteOptions, Written, write};
    use crate::diagnostic::{Diagnostic, DiagnosticKind};
    use crate::{ImportReport, ImportedBlock, import, kept, syntax};

    /// Reads a CIF file's bytes. Invalid UTF-8 is replaced and reported.
    pub fn read(bytes: &[u8]) -> ImportReport {
        read_with(bytes, Tolerances::default())
    }

    /// [`read`] with other tolerances than the defaults: the structures are
    /// normalized with them and keep them.
    pub fn read_with(bytes: &[u8], tolerances: Tolerances) -> ImportReport {
        let (text, valid) = match std::str::from_utf8(bytes) {
            Ok(text) => (Cow::Borrowed(text), true),
            Err(_) => (String::from_utf8_lossy(bytes), false),
        };
        let mut report = read_str_with(&text, tolerances);
        if !valid {
            report
                .diagnostics
                .insert(0, Diagnostic::new(DiagnosticKind::NotUtf8, None));
        }
        report
    }

    /// Reads CIF text.
    pub fn read_str(text: &str) -> ImportReport {
        read_str_with(text, Tolerances::default())
    }

    fn read_str_with(text: &str, tolerances: Tolerances) -> ImportReport {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let document = syntax::parse(text);
        let lines: Vec<&str> = text.lines().collect();
        let mut diagnostics = document.diagnostics;
        if text.starts_with("#\\#CIF_2.0") {
            diagnostics.insert(0, Diagnostic::new(DiagnosticKind::Cif2, Some(1)));
        }
        let blocks = document
            .blocks
            .iter()
            .map(|block| {
                let (structure, diagnostics, labels) = import::interpret(block, tolerances);
                let mut fields = kept::collect(block, &lines, &labels);
                if let Some(structure) = &structure {
                    fields.retain_atoms(structure);
                }
                ImportedBlock {
                    name: block.name.clone(),
                    line: block.line,
                    structure,
                    diagnostics,
                    fields,
                }
            })
            .collect();
        ImportReport {
            blocks,
            diagnostics,
        }
    }
}
