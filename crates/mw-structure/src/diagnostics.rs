//! Diagnostics as the user reads them: a localized message per kind, and
//! the row that shows one with its severity and source line. Used by the
//! diagnostics tab and the import error dialog.

use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use gpui_kit::*;
use mw_crystal::{SymOpError, SymmetryError};
use mw_crystal_io::{Diagnostic, DiagnosticKind, Severity};

/// Distances in messages, in Å.
fn distance(value: f64) -> String {
    format!("{value:.3}")
}

/// The message for an import diagnostic, in the current language.
pub(crate) fn message(kind: &DiagnosticKind) -> SharedString {
    use DiagnosticKind::*;
    match kind {
        NotUtf8 => tr!("diagnostic.syntax.not_utf8"),
        Cif2 => tr!("diagnostic.syntax.cif2"),
        NoDataBlock => tr!("diagnostic.syntax.no_data_block"),
        ContentOutsideBlock => tr!("diagnostic.syntax.content_outside_block"),
        EmptyBlockName => tr!("diagnostic.syntax.empty_block_name"),
        UnterminatedQuote => tr!("diagnostic.syntax.unterminated_quote"),
        UnterminatedTextField => tr!("diagnostic.syntax.unterminated_text_field"),
        MissingValue { tag } => tr!("diagnostic.syntax.missing_value", tag = tag),
        UnexpectedValue { text } => tr!("diagnostic.syntax.unexpected_value", text = text),
        EmptyLoop => tr!("diagnostic.syntax.empty_loop"),
        LoopValueCount { tags, values } => {
            tr!(
                "diagnostic.syntax.loop_value_count",
                tags = tags,
                values = values
            )
        }
        ReservedWord { word } => tr!("diagnostic.syntax.reserved_word", word = word),
        SaveFrameIgnored { name } => tr!("diagnostic.syntax.save_frame_ignored", name = name),
        DuplicateTag { tag } => tr!("diagnostic.syntax.duplicate_tag", tag = tag),
        AliasConflict { used, ignored } => {
            tr!(
                "diagnostic.syntax.alias_conflict",
                used = used,
                ignored = ignored
            )
        }
        NoStructure => tr!("diagnostic.block.no_structure"),
        InvalidNumber { tag, text } => {
            tr!("diagnostic.value.invalid_number", tag = tag, text = text)
        }
        MissingCellParameter { tag } => tr!("diagnostic.cell.missing_parameter", tag = tag),
        AngleDefaulted { tag } => tr!("diagnostic.cell.angle_defaulted", tag = tag),
        InvalidCell => tr!("diagnostic.cell.invalid"),
        InvalidSymop { text, error } => match error {
            SymOpError::ComponentCount(count) => tr!(
                "diagnostic.symmetry.invalid_operation.component_count",
                text = text,
                count = count
            ),
            SymOpError::NonIntegerCoefficient => tr!(
                "diagnostic.symmetry.invalid_operation.non_integer",
                text = text
            ),
            SymOpError::UnrecognizedDecimal(value) => tr!(
                "diagnostic.symmetry.invalid_operation.decimal",
                text = text,
                value = value
            ),
            SymOpError::Syntax => {
                tr!("diagnostic.symmetry.invalid_operation.syntax", text = text)
            }
        },
        DuplicateSymop { text } => tr!("diagnostic.symmetry.duplicate_operation", text = text),
        InvalidSymmetry { error, operations } => {
            let op = |i: usize| operations.get(i).cloned().unwrap_or_default();
            match error {
                SymmetryError::MissingIdentity => tr!("diagnostic.symmetry.missing_identity"),
                SymmetryError::Singular(_) => tr!("diagnostic.symmetry.singular", op = op(0)),
                SymmetryError::Duplicate(..) => {
                    tr!("diagnostic.symmetry.duplicate_operation", text = op(1))
                }
                SymmetryError::NotClosed(..) => {
                    tr!("diagnostic.symmetry.not_closed", a = op(0), b = op(1))
                }
            }
        }
        MissingSymops { name: Some(name) } => {
            tr!("diagnostic.symmetry.missing_operations", name = name)
        }
        MissingSymops { name: None } => tr!("diagnostic.symmetry.missing_operations_and_group"),
        InvalidItNumber { text } => tr!("diagnostic.symmetry.invalid_it_number", text = text),
        MissingAtomSites => tr!("diagnostic.atom.missing_sites"),
        MissingAtomTag { tag } => tr!("diagnostic.atom.missing_tag", tag = tag),
        CartesianOnly => tr!("diagnostic.atom.cartesian_only"),
        MissingAtomValue { label, tag } => {
            tr!("diagnostic.atom.missing_value", label = label, tag = tag)
        }
        DuplicateLabel { label } => tr!("diagnostic.atom.duplicate_label", label = label),
        ElementFromLabel { count } => tr!("diagnostic.atom.element_from_label", count = count),
        UnknownElement { label, symbol } => {
            tr!(
                "diagnostic.atom.unknown_element",
                label = label,
                symbol = symbol
            )
        }
        InvalidOccupancy { label, value } => {
            tr!(
                "diagnostic.atom.invalid_occupancy",
                label = label,
                value = value
            )
        }
        OccupancyAboveOne { label, value } => {
            tr!(
                "diagnostic.atom.occupancy_above_one",
                label = label,
                value = value
            )
        }
        OccupancyAssumed { count } => tr!("diagnostic.atom.occupancy_assumed", count = count),
        BisoConverted { count } => tr!("diagnostic.atom.biso_converted", count = count),
        DummyAtomSkipped { label } => tr!("diagnostic.atom.dummy_skipped", label = label),
        AnisoUnknownLabel { label } => tr!("diagnostic.atom.aniso_unknown_label", label = label),
        CloseImages { label, distance: d } => tr!(
            "diagnostic.atom.close_images",
            label = label,
            distance = distance(*d)
        ),
        MultiplicityMismatch {
            label,
            file,
            expanded,
        } => tr!(
            "diagnostic.atom.multiplicity_mismatch",
            label = label,
            file = file,
            expanded = expanded
        ),
        OverlappingSites { a, b } => tr!("diagnostic.atom.overlapping_sites", a = a, b = b),
        SharedPosition { a, b } => tr!("diagnostic.atom.shared_position", a = a, b = b),
        ShortContact { a, b, distance: d } => tr!(
            "diagnostic.atom.short_contact",
            a = a,
            b = b,
            distance = distance(*d)
        ),
        _ => kind.code().into(),
    }
}

/// One diagnostic: severity icon, message, and the source line when it has
/// one.
#[derive(IntoElement)]
pub(crate) struct DiagnosticRow {
    severity: Severity,
    message: SharedString,
    line: Option<u32>,
}

impl DiagnosticRow {
    pub fn new(severity: Severity, message: SharedString, line: Option<u32>) -> Self {
        Self {
            severity,
            message,
            line,
        }
    }

    pub fn from_import(diagnostic: &Diagnostic) -> Self {
        Self::new(
            diagnostic.severity(),
            message(diagnostic.kind()),
            diagnostic.line(),
        )
    }
}

impl RenderOnce for DiagnosticRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (icon, color) = match self.severity {
            Severity::Error => (IconName::CircleX, theme.danger),
            Severity::Warning => (IconName::TriangleAlert, theme.warning),
            Severity::Info => (IconName::Info, theme.info),
        };
        h_flex()
            .items_start()
            .gap_2()
            .px_2()
            .py_1p5()
            .text_sm()
            .child(
                div()
                    .pt_0p5()
                    .text_color(color)
                    .child(Icon::new(icon).small()),
            )
            .child(div().flex_1().min_w_0().child(self.message))
            .children(self.line.map(|line| {
                div()
                    .flex_none()
                    .text_xs()
                    .pt_0p5()
                    .text_color(theme.muted_foreground)
                    .child(tr!("diagnostic.line", line = line))
            }))
    }
}
