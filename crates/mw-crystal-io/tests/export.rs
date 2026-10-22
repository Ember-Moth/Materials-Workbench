//! Export: every fixture survives a CIF round trip (import → export →
//! import), kept fields are written back, and POSCAR and extended XYZ
//! match hand-written files.

mod common;

use std::path::Path;

use mw_crystal::{
    CellParams, Lattice, Measured, Occupant, Site, SiteId, Species, Structure, SymOp, Symmetry,
};
use mw_crystal_io::{ExportError, ImportReport, cif, poscar, xyz};

fn fixture(name: &str) -> ImportReport {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    cif::read(&std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

/// The first structure of a fixture.
fn structure(name: &str) -> Structure {
    let report = fixture(name);
    report.structures().next().expect("a structure").1.clone()
}

#[test]
fn every_fixture_survives_a_round_trip() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".cif"))
        .collect();
    names.sort();
    let mut structures = 0;
    for name in &names {
        let report = fixture(name);
        for (block, structure) in report.structures() {
            if let Err(mismatch) = common::round_trip(block, structure) {
                panic!(
                    "{name}, data_{}: {}: {}",
                    block.name, mismatch.category, mismatch.detail
                );
            }
            structures += 1;
        }
    }
    // malformed.cif has none; multi_block.cif has two.
    assert_eq!(structures, 8);
}

#[test]
fn space_group_and_uncertainties_are_written() {
    let report = fixture("bst_su_aniso.cif");
    let (block, structure) = report.structures().next().unwrap();
    let text = cif::write(structure, &block.write_options(structure)).text;
    for line in [
        "data_BST",
        "_chemical_name_systematic         'Barium strontium titanate'",
        "_cell_length_a                    3.9475(3)",
        "_cell_angle_alpha                 90",
        "_chemical_formula_sum             'Ba0.5 O3 Sr0.5 Ti'",
        "_cell_formula_units_Z             1",
        "_space_group_IT_number            221",
        "_space_group_name_H-M_alt         'P m -3 m'",
        "_space_group_name_Hall            '-P 4 2 3'",
        "Ba1 Ba2+ 1 a 0   0   0   0.50(2) 0.0072 Uani",
        "O1  O2-  3 c 0.5 0.5 0   1       0.011  Uani",
        "O1  0.0062(5) 0.0062(5) 0.0206(9) 0 0 0",
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "{line:?} missing in\n{text}"
        );
    }
    // Derived fields of the source are regenerated, not repeated.
    assert_eq!(text.matches("_chemical_formula_sum").count(), 1);
}

#[test]
fn coordinate_uncertainties_follow_the_site_symmetry() {
    let report = cif::read_str(
        "data_rutile\n_space_group_IT_number 136\n\
         _cell_length_a 4.5937(5)\n_cell_length_b 4.5937(5)\n_cell_length_c 2.9587(3)\n\
         _cell_angle_alpha 90\n_cell_angle_beta 90\n_cell_angle_gamma 90\n\
         loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         Ti1 0 0 0.0000(2)\nO1 0.30478(12) 0.30478(12) 0\n",
    );
    let (block, structure) = report.structures().next().unwrap();
    let su = structure.sites[1].position_su[0].unwrap();
    assert!((su - 0.000_12).abs() < 1e-15);
    let written = common::round_trip(block, structure).unwrap_or_else(|m| panic!("{m:?}"));
    // y follows x and keeps its s.u.; the fixed z of Ti1 loses its own.
    assert!(
        written
            .text
            .contains("O1  O  4 f 0.30478(12) 0.30478(12) 0 1")
    );
    assert!(
        written
            .text
            .contains("Ti1 Ti 2 a 0           0           0 1")
    );

    // Only x has an s.u. here: y and z follow it and are written as
    // precisely, so that snapping their average when read back gives the
    // same value.
    let report = cif::read_str(
        "data_x\n_space_group_name_H-M_alt 'P 21 3'\n\
         _cell_length_a 17.272(3)\n_cell_length_b 17.272\n_cell_length_c 17.272\n\
         loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         Tl1 -0.54206(6) -0.5421 -0.5421\n",
    );
    let (block, structure) = report.structures().next().unwrap();
    let written = common::round_trip(block, structure).unwrap_or_else(|m| panic!("{m:?}"));
    assert!(
        written
            .text
            .contains("Tl1 Tl 4 a -0.54209(6) -0.54209 -0.54209 1"),
        "{}",
        written.text
    );
}

#[test]
fn kept_fields_are_written_back_while_the_structure_is_unchanged() {
    let text = "data_x\n\
                _journal_name_full 'Phys. Rev. B'\n\
                _Custom_Note 'kept'\n\
                _space_group_name_H-M_alt 'P 1'\n\
                _cell_length_a 4\n_cell_length_b 4\n_cell_length_c 4\n\
                _cell_angle_alpha 90\n_cell_angle_beta 90\n_cell_angle_gamma 90\n\
                loop_\n_atom_type_symbol\n_atom_type_oxidation_number\nNa1+ 1\n\
                loop_\n_atom_site_label\n_atom_site_type_symbol\n\
                _atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
                _atom_site_calc_flag\n\
                Na1 Na1+ 0 0 0 d\n";
    let report = cif::read_str(text);
    let (block, structure) = report.structures().next().unwrap();
    let written = common::round_trip(block, structure).unwrap_or_else(|m| panic!("{m:?}"));
    for line in [
        "_journal_name_full                'Phys. Rev. B'",
        "_Custom_Note                      kept",
        "_atom_type_oxidation_number",
        "Na1+ 1",
        "_atom_site_calc_flag",
        "Na1 Na1+ 1 a 0 0 0 1 d",
    ] {
        assert!(
            written.text.lines().any(|l| l == line),
            "{line:?} missing in\n{}",
            written.text
        );
    }

    // After an edit, unrecognized fields no longer describe the structure.
    let mut edited = structure.clone();
    edited.sites[0].position = [0.1, 0.0, 0.0];
    let changed = cif::write(&edited, &block.write_options(&edited));
    assert_eq!(
        changed.dropped,
        [
            "_Custom_Note",
            "_atom_type_symbol",
            "_atom_type_oxidation_number",
            "_atom_site_calc_flag"
        ]
    );
    assert!(changed.text.contains("_journal_name_full"));
    assert!(!changed.text.contains("_atom_type_symbol"));
    assert!(!changed.text.contains("_Custom_Note"));
}

/// A structure with one atom per operation-set orbit, for testing names.
fn named(symmetry: Symmetry, params: CellParams) -> String {
    let site = Site::new(
        SiteId(1),
        [0.1, 0.2, 0.3],
        vec![Occupant::new(
            "C1",
            Species::element("C"),
            Measured::exact(1.0),
        )],
    );
    let structure = Structure::new(Lattice::new(params), symmetry, vec![site]);
    cif::write(&structure, &cif::WriteOptions::new("x")).text
}

fn generated(generators: &[&str]) -> Symmetry {
    let generators: Vec<SymOp> = generators
        .iter()
        .map(|g| SymOp::parse(g).unwrap())
        .collect();
    Symmetry::generate(&generators).unwrap()
}

#[test]
fn names_of_other_settings() {
    let monoclinic = CellParams::new(5.0, 6.0, 7.0, 90.0, 90.0, 100.0);
    let text = named(Symmetry::from_hm_symbol("P 1 1 21/a").unwrap(), monoclinic);
    assert!(text.contains("_space_group_name_H-M_alt         'P 1 1 21/a'"));
    assert!(text.contains("_space_group_name_Hall            '-P 2ac'"));

    let cubic = CellParams::new(8.0, 8.0, 8.0, 90.0, 90.0, 90.0);
    let text = named(Symmetry::from_hm_symbol("F d -3 m :1").unwrap(), cubic);
    assert!(text.contains("_space_group_name_H-M_alt         'F d -3 m :1'"));

    // P2₁/c with its origin moved: no tabulated symbol names these
    // operations or Wyckoff letters, so only the number is written.
    let monoclinic = CellParams::new(5.0, 6.0, 7.0, 90.0, 100.0, 90.0);
    let text = named(
        generated(&["-x,y+1/2,-z+1/2", "-x+1/2,-y+1/2,-z+1/2"]),
        monoclinic,
    );
    assert!(text.contains("_space_group_IT_number            14"));
    assert!(!text.contains("_space_group_name_H-M_alt"));
    assert!(!text.contains("_space_group_name_Hall"));
    assert!(!text.contains("_atom_site_Wyckoff_symbol"));
    // Read back, the operations name the same type.
    let report = cif::read_str(&text);
    assert!(
        report
            .all_diagnostics()
            .all(|d| d.code() != "symmetry.name_mismatch")
    );
}

const RUTILE_POSCAR: &str = "\
rutile
1.0
        4.5940000000        0.0000000000        0.0000000000
        0.0000000000        4.5940000000        0.0000000000
        0.0000000000        0.0000000000        2.9590000000
   Ti    O
    2    4
Direct
    0.0000000000    0.0000000000    0.0000000000
    0.5000000000    0.5000000000    0.5000000000
    0.3048000000    0.3048000000    0.0000000000
    0.1952000000    0.8048000000    0.5000000000
    0.6952000000    0.6952000000    0.0000000000
    0.8048000000    0.1952000000    0.5000000000
";

const RUTILE_XYZ: &str = "\
6
Lattice=\"4.5940000000 0.0000000000 0.0000000000 0.0000000000 4.5940000000 0.0000000000 0.0000000000 0.0000000000 2.9590000000\" Properties=species:S:1:pos:R:3 pbc=\"T T T\"
Ti    0.0000000000    0.0000000000    0.0000000000
Ti    2.2970000000    2.2970000000    1.4795000000
O     1.4002512000    1.4002512000    0.0000000000
O     0.8967488000    3.6972512000    1.4795000000
O     3.1937488000    3.1937488000    0.0000000000
O     3.6972512000    0.8967488000    1.4795000000
";

/// Hexagonal: b has a negative x component; Zn and O sit on 1/3, 2/3.
const ZINCITE_POSCAR: &str = "\
ZnO, zincite
1.0
        3.2498000000        0.0000000000        0.0000000000
       -1.6249000000        2.8144093572        0.0000000000
        0.0000000000        0.0000000000        5.2066000000
   Zn    O
    2    2
Direct
    0.3333333333    0.6666666667    0.0000000000
    0.6666666667    0.3333333333    0.5000000000
    0.3333333333    0.6666666667    0.3819000000
    0.6666666667    0.3333333333    0.8819000000
";

const ZINCITE_XYZ: &str = "\
4
Lattice=\"3.2498000000 0.0000000000 0.0000000000 -1.6249000000 2.8144093572 0.0000000000 0.0000000000 0.0000000000 5.2066000000\" Properties=species:S:1:pos:R:3 pbc=\"T T T\"
Zn    0.0000000000    1.8762729048    0.0000000000
Zn    1.6249000000    0.9381364524    2.6033000000
O     0.0000000000    1.8762729048    1.9884005400
O     1.6249000000    0.9381364524    4.5917005400
";

#[test]
fn poscar_and_xyz_match_hand_written_files() {
    let rutile = structure("rutile.cif");
    assert_eq!(poscar::write(&rutile, "rutile").unwrap(), RUTILE_POSCAR);
    assert_eq!(xyz::write(&rutile).unwrap(), RUTILE_XYZ);
    let zincite = structure("zincite_legacy.cif");
    // A line break in the comment would end the comment line.
    assert_eq!(
        poscar::write(&zincite, "ZnO,\nzincite").unwrap(),
        ZINCITE_POSCAR
    );
    assert_eq!(xyz::write(&zincite).unwrap(), ZINCITE_XYZ);
}

#[test]
fn atom_lists_refuse_what_they_cannot_hold() {
    // Ba/Sr share a site.
    let bst = structure("bst_su_aniso.cif");
    let partial = ExportError::PartialOccupancy {
        labels: vec!["Ba1".into()],
    };
    assert_eq!(poscar::check(&bst), Err(partial.clone()));
    assert_eq!(poscar::write(&bst, ""), Err(partial.clone()));
    assert_eq!(xyz::write(&bst), Err(partial));

    let report = cif::read_str(
        "data_x\n_space_group_name_H-M_alt 'P 1'\n\
         _cell_length_a 4\n_cell_length_b 4\n_cell_length_c 4\n\
         loop_\n_atom_site_label\n_atom_site_type_symbol\n\
         _atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         Q1 Q 0 0 0\nNa1 Na 0.5 0.5 0.5\n",
    );
    let unknown = report.structures().next().unwrap().1;
    assert_eq!(
        xyz::check(unknown),
        Err(ExportError::UnknownElement {
            labels: vec!["Q1".into()]
        })
    );
}
