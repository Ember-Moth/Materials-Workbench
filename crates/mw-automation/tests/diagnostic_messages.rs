//! Every POSCAR import finding has its own English sentence for models,
//! not the generic fallback.

use mw_automation::diagnostic_message;
use mw_crystal_io::{Diagnostic, DiagnosticKind};

#[test]
fn poscar_diagnostics_have_messages() {
    use DiagnosticKind::*;
    let text = || "x".to_string();
    for kind in [
        PoscarTruncated,
        InvalidScale { text: text() },
        UnequalScaleFactors {
            factors: [1.0, 1.0, 2.0],
        },
        InvalidLatticeVector { text: text() },
        DegenerateLattice,
        LeftHandedLattice,
        MissingElements { species: 2 },
        InvalidAtomCounts { text: text() },
        SpeciesCountMismatch {
            elements: 1,
            counts: 2,
        },
        SelectiveDynamicsIgnored,
        InvalidCoordinateMode { text: text() },
        InvalidCoordinates { text: text() },
        MissingCoordinates {
            expected: 2,
            found: 1,
        },
        TrailingContentIgnored,
    ] {
        let message = diagnostic_message(&Diagnostic::new(kind, Some(1)));
        assert!(!message.starts_with("Import finding"), "{message}");
    }
}
