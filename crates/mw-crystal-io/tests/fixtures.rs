//! Imports every fixture in `tests/fixtures` and checks the structures and
//! diagnostics, including the source lines the diagnostics point at.

use std::collections::HashSet;
use std::path::Path;

use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{Adp, Derived, EditMode, Setting, SymOp};
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

/// The identified space group number and each site's label, Wyckoff
/// position and coordinate form, e.g. `O1 4f x, x, 0`.
fn symmetry(derived: &Derived, structure: &mw_crystal::Structure) -> (u16, Vec<String>) {
    let analysis = &derived.symmetry;
    let number = analysis.space_group.as_ref().expect("identified").number;
    let sites = structure
        .sites
        .iter()
        .map(|site| {
            let symmetry = derived.site_symmetry(site.id).expect("analyzed");
            let wyckoff = symmetry.wyckoff.as_ref().expect("tabulated");
            format!("{} {wyckoff} {}", site.label(), symmetry.form)
        })
        .collect();
    (number, sites)
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
    // The same Wyckoff positions as the file's own columns.
    assert_eq!(
        symmetry(&derived, structure),
        (136, vec!["Ti1 2a 0, 0, 0".into(), "O1 4f x, x, 0".into()])
    );
    let space_group = derived.symmetry.space_group.as_ref().unwrap();
    assert_eq!(space_group.setting, Setting::Standard);
    let o1 = derived.site_symmetry(structure.sites[1].id).unwrap();
    assert_eq!(o1.wyckoff.as_ref().unwrap().site_symmetry, "m.2m");
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
    assert_eq!(
        symmetry(&derived, structure),
        (
            221,
            vec![
                "Sr1 1a 0, 0, 0".into(),
                "Ti1 1b 1/2, 1/2, 1/2".into(),
                "O1 3c 0, 1/2, 1/2".into()
            ]
        )
    );
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
    let derived = Derived::compute(structure);
    assert_eq!(derived.atoms_in_cell(), 4);
    // 0.33333 is read as 1/3 without a report: the move is below the
    // precision of the written digits.
    assert_eq!(structure.sites[0].position[0], 1.0 / 3.0);
    assert_eq!(
        symmetry(&derived, structure),
        (
            186,
            vec!["Zn1 2b 1/3, 2/3, z".into(), "O1 2b 1/3, 2/3, z".into()]
        )
    );
}

#[test]
fn uncertainties_anisotropic_adps_and_mixed_occupancy() {
    let (text, report) = fixture("bst_su_aniso.cif");
    let structure = report.blocks[0].structure.as_ref().expect("structure");
    let a = structure.lattice.params().a;
    assert_eq!(a.value(), 3.9475);
    assert!((a.su().unwrap() - 0.0003).abs() < 1e-12);

    let ba = &structure.sites[0].occupants[0];
    assert!((ba.occupancy.su().unwrap() - 0.02).abs() < 1e-12);
    let oxygen = &structure.sites[2].occupants[0];
    let Some(Adp::Anisotropic(u)) = oxygen.adp else {
        panic!("anisotropic");
    };
    assert_eq!(u[2].value(), 0.0206);
    assert!((u[2].su().unwrap() - 0.0009).abs() < 1e-12);
    assert_eq!(u[3].su(), None);

    // Ba/Sr mixed occupancy is two rows at one position: one site with
    // two occupants, each keeping its own ADP.
    assert_eq!(
        kinds(&report, 0),
        [(
            DiagnosticKind::MergedOccupancy {
                label: "Sr1".into(),
                into: "Ba1".into()
            },
            Some(line_of(&text, "Sr1 Sr2+"))
        )]
    );
    assert_eq!(report.blocks[0].diagnostics[0].severity(), Severity::Info);
    let labels: Vec<_> = structure.sites.iter().map(|s| s.label()).collect();
    assert_eq!(labels, ["Ba1", "Ti1", "O1"]);
    let mixed = &structure.sites[0].occupants;
    assert_eq!(mixed.len(), 2);
    assert_eq!(mixed[1].species.element, "Sr");
    assert!(matches!(mixed[1].adp, Some(Adp::Anisotropic(_))));

    let derived = Derived::compute(structure);
    assert_eq!(derived.atoms_in_cell(), 5);
    assert_eq!(
        symmetry(&derived, structure),
        (
            221,
            vec![
                "Ba1 1a 0, 0, 0".into(),
                "Ti1 1b 1/2, 1/2, 1/2".into(),
                "O1 3c 1/2, 1/2, 0".into()
            ]
        )
    );
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

    // No operations listed: they come from the H-M symbol.
    let copper = report.blocks[2].structure.as_ref().expect("structure");
    assert_eq!(
        kinds(&report, 2),
        [(
            DiagnosticKind::SymopsFromName {
                name: "F m -3 m".into()
            },
            Some(line_of(&text, "data_copper"))
        )]
    );
    assert_eq!(copper.symmetry.operations().len(), 192);
    let derived = Derived::compute(copper);
    assert_eq!(derived.atoms_in_cell(), 4);
    assert_eq!(
        symmetry(&derived, copper),
        (225, vec!["Cu1 4a 0, 0, 0".into()])
    );

    assert_eq!(report.structures().count(), 2);
    assert_eq!(report.failed_blocks().count(), 0);
}

#[test]
fn a_structure_written_in_p1_is_recognized() {
    let (_, report) = fixture("halite_p1.cif");
    let block = &report.blocks[0];
    assert!(block.diagnostics.is_empty(), "{:?}", block.diagnostics);
    let structure = block.structure.as_ref().expect("structure");
    assert_eq!(structure.edit_mode(), EditMode::P1);
    assert_eq!(structure.sites.len(), 8);

    let derived = Derived::compute(structure);
    let analysis = &derived.symmetry;
    // The operations are P1, every atom on the general position 1a...
    assert_eq!(analysis.space_group.as_ref().map(|s| s.number), Some(1));
    assert!(
        structure
            .sites
            .iter()
            .all(|s| derived.site_symmetry(s.id).unwrap().order == 1)
    );
    // ...and the atoms have the symmetry of rock salt: Na on 4a, Cl on 4b.
    let detected = analysis.detected.as_ref().expect("searched");
    assert_eq!((detected.number, detected.hm_symbol), (225, "F m -3 m"));
    assert_eq!(detected.operations, 192);
    let letters: Vec<char> = detected.wyckoffs.iter().map(|(_, w)| *w).collect();
    assert_eq!(letters, ['a', 'a', 'a', 'a', 'b', 'b', 'b', 'b']);
}

#[test]
fn sites_near_special_positions_are_snapped_and_merged() {
    let (text, report) = fixture("rutile_near_special.cif");
    let block = &report.blocks[0];
    let structure = block.structure.as_ref().expect("structure");
    let line = |prefix| Some(line_of(&text, prefix));
    let found: Vec<_> = block
        .diagnostics
        .iter()
        .map(|d| (d.code(), d.line(), d.severity()))
        .collect();
    assert_eq!(
        found,
        [
            (
                "symmetry.operations_from_name",
                line("data_"),
                Severity::Info
            ),
            ("atom.snapped", line("Ti1"), Severity::Info),
            ("atom.merged_occupancy", line("V1"), Severity::Info),
            ("atom.snapped", line("O1"), Severity::Info),
            ("atom.redundant_site", line("O2"), Severity::Info),
        ]
    );
    let DiagnosticKind::SnappedToSpecialPosition {
        label,
        form,
        displacement,
    } = block.diagnostics[3].kind()
    else {
        panic!("{:?}", block.diagnostics[3]);
    };
    assert_eq!((label.as_str(), form.as_str()), ("O1", "x, x, 0"));
    assert!((displacement - 0.00323).abs() < 1e-5, "{displacement}");
    assert!(matches!(
        block.diagnostics[4].kind(),
        DiagnosticKind::RedundantSite { label, duplicate_of } if label == "O2" && duplicate_of == "O1"
    ));

    // Two sites remain, exactly on 2a and 4f.
    assert_eq!(structure.sites.len(), 2);
    assert_eq!(structure.sites[0].position, [0.0, 0.0, 0.0]);
    let elements: Vec<_> = structure.sites[0]
        .occupants
        .iter()
        .map(|o| o.species.element.as_str())
        .collect();
    assert_eq!(elements, ["Ti", "V"]);
    let o1 = structure.sites[1].position;
    assert_eq!(o1[0], o1[1]);
    assert!((o1[0] - 0.3048).abs() < 1e-12);
    assert_eq!(o1[2], 0.0);

    let derived = Derived::compute(structure);
    assert_eq!(derived.atoms_in_cell(), 6);
    assert_eq!(
        symmetry(&derived, structure),
        (136, vec!["Ti1 2a 0, 0, 0".into(), "O1 4f x, x, 0".into()])
    );
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

/// Each block's diagnostics as (code, line).
fn codes(report: &ImportReport, block: usize) -> Vec<(&'static str, Option<u32>)> {
    report.blocks[block]
        .diagnostics
        .iter()
        .map(|d| (d.code(), d.line()))
        .collect()
}

#[test]
fn shelxl_site_symmetry_orders_are_no_mismatch() {
    let (text, report) = fixture("shelxl_multiplicity.cif");
    assert_eq!(report.structures().count(), 3);
    // SHELXL-97 values under the multiplicity tag fit as orders.
    assert_eq!(codes(&report, 0), []);
    // An explicit order, and multiplicities as in International Tables:
    // only O1's wrong value is reported, as the multiplicity it implies.
    let o1_rows: Vec<u32> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with("O1 "))
        .map(|(i, _)| i as u32 + 1)
        .collect();
    for (block, line) in [(1, o1_rows[1]), (2, o1_rows[2])] {
        assert_eq!(
            kinds(&report, block),
            [(
                DiagnosticKind::MultiplicityMismatch {
                    label: "O1".into(),
                    file: 1,
                    expanded: 2
                },
                Some(line)
            )]
        );
    }
}

#[test]
fn entries_without_coordinates_have_no_structure() {
    let (text, report) = fixture("placeholder_entry.cif");
    assert_eq!(report.structures().count(), 0);
    assert_eq!(
        codes(&report, 0),
        [
            (
                "symmetry.missing_operations",
                Some(line_of(&text, "data_cell_only"))
            ),
            ("atom.missing_sites", Some(line_of(&text, "loop_"))),
        ]
    );
    // Without a cell, the all-`?` row is no structure at all.
    assert_eq!(
        codes(&report, 1),
        [("block.no_structure", Some(line_of(&text, "data_no_cell")))]
    );
    assert_eq!(report.blocks[1].diagnostics[0].severity(), Severity::Info);
}

#[test]
fn dummy_rows_may_leave_out_the_label() {
    let (text, report) = fixture("dummy_row.cif");
    let block = &report.blocks[0];
    assert_eq!(block.structure.as_ref().expect("structure").sites.len(), 3);
    assert_eq!(
        codes(&report, 0),
        [
            (
                "symmetry.operations_from_name",
                Some(line_of(&text, "data_"))
            ),
            ("atom.dummy_skipped", Some(line_of(&text, ". . ."))),
        ]
    );
}

#[test]
fn repeated_labels_are_renamed() {
    let (text, report) = fixture("duplicate_labels.cif");
    let block = &report.blocks[0];
    let structure = block.structure.as_ref().expect("structure");
    let labels: Vec<_> = structure.sites.iter().map(|s| s.label()).collect();
    assert_eq!(
        labels,
        [
            "N1", "H1A", "H1B", "C1", "H1A_2", "H1B_2", "C2", "C2_3", "C2_2"
        ]
    );
    let renamed: Vec<_> = block
        .diagnostics
        .iter()
        .map(|d| match d.kind() {
            DiagnosticKind::DuplicateLabel { label, renamed } => {
                assert_eq!(d.severity(), Severity::Warning);
                (label.as_str(), renamed.as_str(), d.line())
            }
            other => panic!("{other:?}"),
        })
        .collect();
    let row = |prefix: &str, nth: usize| {
        text.lines()
            .enumerate()
            .filter(|(_, l)| l.starts_with(prefix))
            .nth(nth)
            .map(|(i, _)| i as u32 + 1)
    };
    assert_eq!(
        renamed,
        [
            ("H1A", "H1A_2", row("H1A ", 1)),
            ("H1B", "H1B_2", row("H1B ", 1)),
            ("C2", "C2_3", row("C2 C", 1)),
        ]
    );
    // The two anisotropic C2 rows go to the two C2 sites in order.
    let u11 = |site: usize| match structure.sites[site].occupants[0].adp {
        Some(Adp::Anisotropic(u)) => u[0].value(),
        ref other => panic!("{other:?}"),
    };
    assert_eq!((u11(6), u11(7)), (0.011, 0.021));
}

#[test]
fn atoms_with_zero_occupancy_are_dropped() {
    let (text, report) = fixture("zero_occupancy.cif");
    let block = &report.blocks[0];
    let structure = block.structure.as_ref().expect("structure");
    let labels: Vec<_> = structure.sites.iter().map(|s| s.label()).collect();
    assert_eq!(labels, ["C5", "C6", "C5A"]);
    // Its anisotropic row goes with it: no unknown-label warning.
    assert_eq!(
        kinds(&report, 0),
        [(
            DiagnosticKind::InvalidOccupancy {
                label: "C6A".into(),
                value: 0.0
            },
            Some(line_of(&text, "C6A C"))
        )]
    );
    assert_eq!(block.diagnostics[0].severity(), Severity::Warning);
}

#[test]
fn damaged_adp_values_do_not_cost_the_structure() {
    let (text, report) = fixture("truncated_su.cif");
    let block = &report.blocks[0];
    let structure = block.structure.as_ref().expect("structure");
    // `0.0029(18` (U12, the last column) at the end of a cut-off line
    // reads with its s.u.
    let Some(Adp::Anisotropic(u)) = &structure.sites[0].occupants[0].adp else {
        panic!("anisotropic");
    };
    assert_eq!(u[3].value(), 0.0029);
    assert!((u[3].su().unwrap() - 0.0018).abs() < 1e-15);
    // A typo drops that row's ADPs only: C42 keeps its U_iso.
    assert_eq!(
        kinds(&report, 0),
        [(
            DiagnosticKind::InvalidAdp {
                label: "C42".into(),
                tag: "_atom_site_aniso_u_23".into(),
                text: "0..0042(15)".into()
            },
            Some(line_of(&text, "C42 0.015"))
        )]
    );
    assert_eq!(block.diagnostics[0].severity(), Severity::Warning);
    let Some(Adp::Isotropic(u)) = &structure.sites[2].occupants[0].adp else {
        panic!("isotropic");
    };
    assert_eq!(u.value(), 0.0141);
}

#[test]
fn operations_from_a_hall_symbol_with_a_double_prime() {
    let (text, report) = fixture("corundum_hall.cif");
    let block = &report.blocks[0];
    assert_eq!(
        kinds(&report, 0),
        [(
            DiagnosticKind::SymopsFromName {
                name: "-R 3 2\"c".into()
            },
            Some(line_of(&text, "data_"))
        )]
    );
    let structure = block.structure.as_ref().expect("structure");
    assert_eq!(structure.symmetry.operations().len(), 36);
    let derived = Derived::compute(structure);
    assert_eq!(derived.atoms_in_cell(), 30);
    assert_eq!(
        symmetry(&derived, structure),
        (
            167,
            vec!["Al1 12c 0, 0, z".into(), "O1 18e x, 0, 1/4".into()]
        )
    );
}

#[test]
fn current_and_legacy_names_conflict_only_in_meaning() {
    let (text, report) = fixture("alias_spellings.cif");
    assert_eq!(report.structures().count(), 2);
    assert_eq!(codes(&report, 0), []);
    let legacy = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with("_symmetry_space_group_name_H-M"))
        .nth(1)
        .map(|(i, _)| i as u32 + 1);
    assert_eq!(
        kinds(&report, 1),
        [
            (
                DiagnosticKind::SymopsFromName {
                    name: "F d -3 m".into()
                },
                Some(line_of(&text, "data_other_origin"))
            ),
            (
                DiagnosticKind::AliasConflict {
                    used: "_space_group_name_h-m_alt".into(),
                    ignored: "_symmetry_space_group_name_h-m".into()
                },
                legacy
            ),
        ]
    );
    // Origin choice 2: Si on 8a at 1/8, 1/8, 1/8.
    let structure = report.blocks[1].structure.as_ref().expect("structure");
    let derived = Derived::compute(structure);
    assert_eq!(
        symmetry(&derived, structure),
        (227, vec!["Si1 8a 1/8, 1/8, 1/8".into()])
    );
}
