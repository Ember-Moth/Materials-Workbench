//! Imports every fixture in `tests/fixtures` and checks the structures and
//! diagnostics, including the source lines the diagnostics point at.

use std::collections::HashSet;
use std::path::Path;

use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{Adp, Derived, EditMode, SymOp};
use mw_crystal_io::{DiagnosticKind, ImportReport, Severity, cif};

fn fixture(name: &str) -> (String, ImportReport) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    (String::from_utf8(bytes.clone()).unwrap(), cif::read(&bytes))
}

/// 1-based line of the first line that starts with `prefix`.
fn line_of(text: &str, prefix: &str) -> u32 {
    text.lines()
        .position(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starts with {prefix}")) as u32
        + 1
}

fn kinds(report: &ImportReport, block: usize) -> Vec<(DiagnosticKind, Option<u32>)> {
    report.blocks[block]
        .diagnostics
        .iter()
        .map(|d| (d.kind().clone(), d.line()))
        .collect()
}

fn operations(ops: &[SymOp]) -> HashSet<SymOp> {
    ops.iter().copied().collect()
}

#[test]
fn rutile_matches_the_built_in_sample() {
    let (_, report) = fixture("rutile.cif");
    assert!(report.diagnostics.is_empty());
    assert_eq!(report.blocks.len(), 1);
    let block = &report.blocks[0];
    assert_eq!(block.name, "rutile");
    assert!(block.diagnostics.is_empty(), "{:?}", block.diagnostics);
    let structure = block.structure.as_ref().expect("structure");
    assert_eq!(
        structure.space_group.hm_symbol.as_deref(),
        Some("P 42/m n m")
    );
    assert_eq!(structure.space_group.it_number, Some(136));
    assert_eq!(structure.edit_mode(), EditMode::Symmetric);

    let sample = sample::load(SampleKey::Rutile).structure;
    assert_eq!(
        operations(structure.symmetry.operations()),
        operations(sample.symmetry.operations())
    );
    assert_eq!(structure.lattice, sample.lattice);
    for (imported, built_in) in structure.sites.iter().zip(&sample.sites) {
        assert_eq!(imported.label(), built_in.label());
        assert_eq!(imported.position, built_in.position);
    }

    let ti = &structure.sites[0].occupants[0];
    assert_eq!(ti.species.element, "Ti");
    assert_eq!(ti.species.oxidation, Some(4));
    assert_eq!(ti.species.symbol, "Ti4+");
    assert!(matches!(ti.adp, Some(Adp::Isotropic(u)) if u.value() == 0.006));

    let derived = Derived::compute(structure);
    assert_eq!(derived.atoms_in_cell(), 6);
    assert_eq!(derived.atoms.len(), 15);
    assert!((derived.density.unwrap() - 4.25).abs() < 0.01);
    assert!(derived.symmetry.is_none(), "Wyckoff positions are M2");
}

#[test]
fn strontium_titanate_expands_to_one_formula_unit() {
    let (_, report) = fixture("SrTiO3.cif");
    let block = &report.blocks[0];
    assert!(block.diagnostics.is_empty(), "{:?}", block.diagnostics);
    let structure = block.structure.as_ref().expect("structure");
    assert_eq!(structure.symmetry.operations().len(), 48);
    let derived = Derived::compute(structure);
    let multiplicities: Vec<_> = structure
        .sites
        .iter()
        .map(|s| derived.multiplicity(s.id))
        .collect();
    assert_eq!(multiplicities, [Some(1), Some(1), Some(3)]);
    assert_eq!(derived.atoms.len(), 8 + 1 + 6);
}

#[test]
fn legacy_names_decimals_and_biso() {
    let (text, report) = fixture("zincite_legacy.cif");
    let structure = report.blocks[0].structure.as_ref().expect("structure");
    assert_eq!(structure.space_group.hm_symbol.as_deref(), Some("P 63 m c"));
    assert_eq!(structure.space_group.it_number, Some(186));
    assert_eq!(structure.lattice.params().gamma.value(), 120.0);
    // `z+0.5` is read as the exact translation 1/2.
    let screw = SymOp::parse("x-y,x,z+1/2").unwrap();
    assert!(structure.symmetry.operations().contains(&screw));
    assert_eq!(structure.symmetry.operations().len(), 12);

    let elements: Vec<_> = structure.sites.iter().map(|s| s.element()).collect();
    assert_eq!(elements, ["Zn", "O"]);
    let Some(Adp::Isotropic(u)) = structure.sites[0].occupants[0].adp else {
        panic!("isotropic");
    };
    assert!((u.value() - 0.57 / (8.0 * std::f64::consts::PI.powi(2))).abs() < 1e-12);

    let atom_loop = line_of(&text, "_atom_site_label") - 1;
    assert_eq!(
        kinds(&report, 0),
        [
            (
                DiagnosticKind::ElementFromLabel { count: 2 },
                Some(atom_loop)
            ),
            (DiagnosticKind::BisoConverted { count: 2 }, Some(atom_loop)),
        ]
    );
    assert_eq!(Derived::compute(structure).atoms_in_cell(), 4);
}

#[test]
fn uncertainties_anisotropic_adps_and_shared_positions() {
    let (text, report) = fixture("bst_su_aniso.cif");
    let structure = report.blocks[0].structure.as_ref().expect("structure");
    let a = structure.lattice.params().a;
    assert_eq!(a.value(), 3.9475);
    assert!((a.su().unwrap() - 0.0003).abs() < 1e-12);

    let ba = &structure.sites[0].occupants[0];
    assert!((ba.occupancy.su().unwrap() - 0.02).abs() < 1e-12);
    let oxygen = &structure.sites[3].occupants[0];
    let Some(Adp::Anisotropic(u)) = oxygen.adp else {
        panic!("anisotropic");
    };
    assert_eq!(u[2].value(), 0.0206);
    assert!((u[2].su().unwrap() - 0.0009).abs() < 1e-12);
    assert_eq!(u[3].su(), None);

    // Ba/Sr mixed occupancy is two rows at one position: reported, kept.
    assert_eq!(
        kinds(&report, 0),
        [(
            DiagnosticKind::SharedPosition {
                a: "Ba1".into(),
                b: "Sr1".into()
            },
            Some(line_of(&text, "Sr1 Sr2+"))
        )]
    );
    assert_eq!(report.blocks[0].diagnostics[0].severity(), Severity::Info);
}

#[test]
fn malformed_file_reports_every_error_with_its_line() {
    let (_, report) = fixture("malformed.cif");
    let block = &report.blocks[0];
    assert!(block.structure.is_none());
    let codes: Vec<_> = block
        .diagnostics
        .iter()
        .map(|d| (d.code(), d.line(), d.severity()))
        .collect();
    assert_eq!(
        codes,
        [
            ("cell.missing_parameter", Some(5), Severity::Error),
            ("syntax.missing_value", Some(9), Severity::Error),
            ("syntax.unterminated_quote", Some(9), Severity::Error),
            ("symmetry.invalid_operation", Some(13), Severity::Error),
            ("symmetry.invalid_operation", Some(14), Severity::Error),
            ("syntax.loop_value_count", Some(15), Severity::Error),
        ]
    );
    assert_eq!(report.failed_blocks().count(), 1);
}

#[test]
fn every_block_is_its_own_structure() {
    let (text, report) = fixture("multi_block.cif");
    let names: Vec<_> = report.blocks.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["global", "CsCl_P1", "copper"]);

    assert!(report.blocks[0].structure.is_none());
    assert_eq!(
        kinds(&report, 0),
        [(
            DiagnosticKind::NoStructure,
            Some(line_of(&text, "data_global"))
        )]
    );

    let cscl = report.blocks[1].structure.as_ref().expect("P1 structure");
    assert_eq!(cscl.edit_mode(), EditMode::P1);
    assert_eq!(cscl.sites.len(), 2);
    assert!(report.blocks[1].diagnostics.is_empty());

    assert!(report.blocks[2].structure.is_none());
    assert_eq!(
        kinds(&report, 2),
        [(
            DiagnosticKind::MissingSymops {
                name: Some("F m -3 m".into())
            },
            Some(line_of(&text, "data_copper"))
        )]
    );

    assert_eq!(report.structures().count(), 1);
    let failed: Vec<_> = report.failed_blocks().map(|b| b.name.as_str()).collect();
    assert_eq!(failed, ["copper"]);
}

#[test]
fn every_fixture_imports_without_panicking() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "cif") {
            let report = cif::read(&std::fs::read(&path).unwrap());
            assert!(!report.blocks.is_empty(), "{}", path.display());
        }
    }
}
