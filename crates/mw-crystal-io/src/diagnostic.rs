//! Import diagnostics (design §8): what was wrong with a file, or what the
//! importer did on the user's behalf, with the source line it concerns.
//!
//! Diagnostics are data, not text: the application turns each kind into a
//! localized message, and batch tools count them by [`DiagnosticKind::code`].

use mw_crystal::{SymOpError, SymmetryError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The data violates a hard constraint; no structure is built from it.
    Error,
    /// Allowed, but the user needs to know.
    Warning,
    /// A change the importer made on the user's behalf.
    Info,
}

/// What a diagnostic reports. Labels and texts are quoted from the file.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum DiagnosticKind {
    // Syntax
    /// The file is not valid UTF-8; invalid bytes were replaced.
    NotUtf8,
    /// The file declares CIF 2.0 and is read as CIF 1.1.
    Cif2,
    NoDataBlock,
    ContentOutsideBlock,
    EmptyBlockName,
    UnterminatedQuote,
    UnterminatedTextField,
    /// A tag with no value after it.
    MissingValue {
        tag: String,
    },
    /// A value that belongs to no tag.
    UnexpectedValue {
        text: String,
    },
    EmptyLoop,
    /// A loop whose value count is not a multiple of its tag count.
    LoopValueCount {
        tags: usize,
        values: usize,
    },
    /// `global_` or `stop_`, which CIF does not allow.
    ReservedWord {
        word: String,
    },
    SaveFrameIgnored {
        name: String,
    },
    /// The tag appears again; the first occurrence is used.
    DuplicateTag {
        tag: String,
    },
    /// Both the current and the legacy name of a field are present with
    /// different values; `used` is read.
    AliasConflict {
        used: String,
        ignored: String,
    },

    // Content
    /// The block has neither cell parameters nor atom sites, e.g. a
    /// publication block; it is skipped.
    NoStructure,
    InvalidNumber {
        tag: String,
        text: String,
    },

    // Cell
    MissingCellParameter {
        tag: String,
    },
    /// A cell angle is absent and taken as 90°.
    AngleDefaulted {
        tag: String,
    },
    /// The cell parameters describe no valid cell.
    InvalidCell,

    // Symmetry
    InvalidSymop {
        text: String,
        error: SymOpError,
    },
    /// The operation is listed again; the repeat is ignored.
    DuplicateSymop {
        text: String,
    },
    /// The operations are not a valid operation set.
    InvalidSymmetry {
        error: SymmetryError,
        /// The operations the error refers to, as written.
        operations: Vec<String>,
    },
    /// No symmetry operations are listed. Generating them from the space
    /// group name comes with the symmetry engine (M2).
    MissingSymops {
        name: Option<String>,
    },
    InvalidItNumber {
        text: String,
    },

    // Atom sites
    MissingAtomSites,
    MissingAtomTag {
        tag: String,
    },
    /// Only Cartesian coordinates are given.
    CartesianOnly,
    MissingAtomValue {
        label: String,
        tag: String,
    },
    DuplicateLabel {
        label: String,
    },
    /// Elements were taken from the labels of this many atoms.
    ElementFromLabel {
        count: usize,
    },
    /// The symbol names no element; the atom is kept as element X.
    UnknownElement {
        label: String,
        symbol: String,
    },
    InvalidOccupancy {
        label: String,
        value: f64,
    },
    OccupancyAboveOne {
        label: String,
        value: f64,
    },
    /// This many atoms have no occupancy and are taken as fully occupied.
    OccupancyAssumed {
        count: usize,
    },
    /// This many atoms give B, converted to U = B / 8π².
    BisoConverted {
        count: usize,
    },
    /// A dummy atom (`_atom_site_calc_flag dum`), not imported.
    DummyAtomSkipped {
        label: String,
    },
    /// An anisotropic row whose label matches no atom site.
    AnisoUnknownLabel {
        label: String,
    },
    /// Equivalent atoms of one fully occupied site closer than the
    /// short-contact distance: the site likely belongs on a special position.
    CloseImages {
        label: String,
        distance: f64,
    },
    /// The file's `_atom_site_symmetry_multiplicity` differs from the
    /// number of atoms the operations generate.
    MultiplicityMismatch {
        label: String,
        file: u32,
        expanded: usize,
    },
    /// Two fully occupied sites at the same position.
    OverlappingSites {
        a: String,
        b: String,
    },
    /// Two partly occupied sites at the same position, i.e. mixed
    /// occupancy; they stay two sites until the importer merges them.
    SharedPosition {
        a: String,
        b: String,
    },
    /// Atoms of two fully occupied sites closer than the short-contact
    /// distance.
    ShortContact {
        a: String,
        b: String,
        distance: f64,
    },
}

impl DiagnosticKind {
    pub fn severity(&self) -> Severity {
        use DiagnosticKind::*;
        match self {
            NoDataBlock
            | UnterminatedQuote
            | UnterminatedTextField
            | MissingValue { .. }
            | UnexpectedValue { .. }
            | EmptyLoop
            | LoopValueCount { .. }
            | ReservedWord { .. }
            | InvalidNumber { .. }
            | MissingCellParameter { .. }
            | InvalidCell
            | InvalidSymop { .. }
            | InvalidSymmetry { .. }
            | MissingSymops { .. }
            | MissingAtomSites
            | MissingAtomTag { .. }
            | CartesianOnly
            | MissingAtomValue { .. }
            | DuplicateLabel { .. }
            | InvalidOccupancy { .. } => Severity::Error,
            NotUtf8
            | Cif2
            | ContentOutsideBlock
            | EmptyBlockName
            | SaveFrameIgnored { .. }
            | DuplicateTag { .. }
            | AliasConflict { .. }
            | DuplicateSymop { .. }
            | InvalidItNumber { .. }
            | UnknownElement { .. }
            | OccupancyAboveOne { .. }
            | AnisoUnknownLabel { .. }
            | CloseImages { .. }
            | MultiplicityMismatch { .. }
            | OverlappingSites { .. }
            | ShortContact { .. } => Severity::Warning,
            NoStructure
            | AngleDefaulted { .. }
            | ElementFromLabel { .. }
            | OccupancyAssumed { .. }
            | BisoConverted { .. }
            | DummyAtomSkipped { .. }
            | SharedPosition { .. } => Severity::Info,
        }
    }

    /// A stable identifier for statistics and tests, e.g.
    /// `syntax.unterminated_quote`.
    pub fn code(&self) -> &'static str {
        use DiagnosticKind::*;
        match self {
            NotUtf8 => "syntax.not_utf8",
            Cif2 => "syntax.cif2",
            NoDataBlock => "syntax.no_data_block",
            ContentOutsideBlock => "syntax.content_outside_block",
            EmptyBlockName => "syntax.empty_block_name",
            UnterminatedQuote => "syntax.unterminated_quote",
            UnterminatedTextField => "syntax.unterminated_text_field",
            MissingValue { .. } => "syntax.missing_value",
            UnexpectedValue { .. } => "syntax.unexpected_value",
            EmptyLoop => "syntax.empty_loop",
            LoopValueCount { .. } => "syntax.loop_value_count",
            ReservedWord { .. } => "syntax.reserved_word",
            SaveFrameIgnored { .. } => "syntax.save_frame_ignored",
            DuplicateTag { .. } => "syntax.duplicate_tag",
            AliasConflict { .. } => "syntax.alias_conflict",
            NoStructure => "block.no_structure",
            InvalidNumber { .. } => "value.invalid_number",
            MissingCellParameter { .. } => "cell.missing_parameter",
            AngleDefaulted { .. } => "cell.angle_defaulted",
            InvalidCell => "cell.invalid",
            InvalidSymop { .. } => "symmetry.invalid_operation",
            DuplicateSymop { .. } => "symmetry.duplicate_operation",
            InvalidSymmetry { .. } => "symmetry.not_a_group",
            MissingSymops { .. } => "symmetry.missing_operations",
            InvalidItNumber { .. } => "symmetry.invalid_it_number",
            MissingAtomSites => "atom.missing_sites",
            MissingAtomTag { .. } => "atom.missing_tag",
            CartesianOnly => "atom.cartesian_only",
            MissingAtomValue { .. } => "atom.missing_value",
            DuplicateLabel { .. } => "atom.duplicate_label",
            ElementFromLabel { .. } => "atom.element_from_label",
            UnknownElement { .. } => "atom.unknown_element",
            InvalidOccupancy { .. } => "atom.invalid_occupancy",
            OccupancyAboveOne { .. } => "atom.occupancy_above_one",
            OccupancyAssumed { .. } => "atom.occupancy_assumed",
            BisoConverted { .. } => "atom.biso_converted",
            DummyAtomSkipped { .. } => "atom.dummy_skipped",
            AnisoUnknownLabel { .. } => "atom.aniso_unknown_label",
            CloseImages { .. } => "atom.close_images",
            MultiplicityMismatch { .. } => "atom.multiplicity_mismatch",
            OverlappingSites { .. } => "atom.overlapping_sites",
            SharedPosition { .. } => "atom.shared_position",
            ShortContact { .. } => "atom.short_contact",
        }
    }
}

/// One finding of an import, at a line of the source file when it has one.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    kind: DiagnosticKind,
    line: Option<u32>,
}

impl Diagnostic {
    pub fn new(kind: DiagnosticKind, line: Option<u32>) -> Self {
        Self { kind, line }
    }

    pub(crate) fn at(kind: DiagnosticKind, line: u32) -> Self {
        Self::new(kind, Some(line))
    }

    pub fn kind(&self) -> &DiagnosticKind {
        &self.kind
    }

    pub fn severity(&self) -> Severity {
        self.kind.severity()
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }

    /// 1-based line in the source file.
    pub fn line(&self) -> Option<u32> {
        self.line
    }
}
