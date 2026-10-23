//! Reading crystal structure files, independent of any UI.
//!
//! [`cif::read`] imports the CIF 1.1 subset described in
//! `docs/design/crystallography-model.md` §9.1: one [`ImportedBlock`] per
//! data block, each with its structure (unless it has errors) and the
//! [`Diagnostic`]s found in it, every one tied to a source line. Structures
//! come out normalized: sites snapped onto special positions, mixed
//! occupancy merged into one site, repeated rows dropped.

mod diagnostic;
mod import;
mod normalize;
mod number;
mod syntax;

pub use diagnostic::{Diagnostic, DiagnosticKind, Severity};

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
}

pub mod cif {
    //! CIF 1.1 import.

    use std::borrow::Cow;

    use mw_crystal::Tolerances;

    use crate::diagnostic::{Diagnostic, DiagnosticKind};
    use crate::{ImportReport, ImportedBlock, import, syntax};

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
        let mut diagnostics = document.diagnostics;
        if text.starts_with("#\\#CIF_2.0") {
            diagnostics.insert(0, Diagnostic::new(DiagnosticKind::Cif2, Some(1)));
        }
        let blocks = document
            .blocks
            .iter()
            .map(|block| {
                let (structure, diagnostics) = import::interpret(block, tolerances);
                ImportedBlock {
                    name: block.name.clone(),
                    line: block.line,
                    structure,
                    diagnostics,
                }
            })
            .collect();
        ImportReport {
            blocks,
            diagnostics,
        }
    }
}
