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
    // 18 structures from 16 files: malformed.cif has none, and several
    // files hold more than one data block. Update when adding a fixture.
    assert_eq!(structures, 18);
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

#[test]
fn followers_with_a_coarser_su_are_written_as_precisely_as_their_pivot() {
    // x has five decimals, y and z (which follow x on 4a) four, each with
    // its own s.u.: snapping averages them. Written with four decimals, y
    // and z would average to another x on every round trip (COD 1100453).
    let report = cif::read_str(
        "data_x\n_space_group_name_H-M_alt 'P 21 3'\n\
         _cell_length_a 12.0\n_cell_length_b 12.0\n_cell_length_c 12.0\n\
         loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         H2 0.30651(5) 0.6939(5) 0.1939(5)\n",
    );
    let (block, structure) = report.structures().next().unwrap();
    let written = common::round_trip(block, structure).unwrap_or_else(|m| panic!("{m:?}"));
    assert!(
        written
            .text
            .contains("H2 H 4 a 0.30624(5) 0.69376(50) 0.19376(50) 1"),
        "{}",
        written.text
    );
}

#[test]
fn special_positions_survive_coarse_uncertainties() {
    // H1 sits on x, x+2/3, 1/12 with an s.u. of 0.010. Rounded to it, as
    // `0.23(1) 0.90(1)`, y − x is no longer 2/3: read back, the atom is
    // 0.05 Å from its image, too far to snap, and is reported as too close
    // to it (COD 4105063).
    let report = cif::read_str(
        "data_x\n_space_group_name_H-M_alt 'R -3 c :H'\n\
         _cell_length_a 8.241\n_cell_length_b 8.241\n_cell_length_c 22.545\n\
         _cell_angle_alpha 90\n_cell_angle_beta 90\n_cell_angle_gamma 120\n\
         loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         H1 0.231(10) 0.898(10) 0.0833\n",
    );
    let (block, structure) = report.structures().next().unwrap();
    let written = common::round_trip(block, structure).unwrap_or_else(|m| panic!("{m:?}"));
    assert!(
        written
            .text
            .contains("H1 H 18 e 0.2312(100) 0.8978(100) 0.0833333333 1"),
        "{}",
        written.text
    );
}

#[test]
fn uncertainties_that_round_up_to_ten_are_written_one_decimal_up() {
    // B = 1.500(77) Å² is U = 0.018997 with an s.u. of 0.000975. One digit
    // at four decimals rounds that s.u. to `(10)`, which reads back as
    // 0.0010 and is written `0.019(1)` the next time (COD 1520296).
    let report = cif::read_str(
        "data_x\n_space_group_name_H-M_alt 'P 1'\n\
         _cell_length_a 5\n_cell_length_b 5\n_cell_length_c 5\n\
         _cell_angle_alpha 90\n_cell_angle_beta 90\n_cell_angle_gamma 90\n\
         loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         _atom_site_B_iso_or_equiv\n\
         O1 0.1 0.2 0.3 1.500(77)\n",
    );
    let (block, structure) = report.structures().next().unwrap();
    let written = common::round_trip(block, structure).unwrap_or_else(|m| panic!("{m:?}"));
    assert!(written.text.contains(" 0.019(1) Uiso"), "{}", written.text);
}

#[test]
fn aniso_columns_of_isotropic_atoms_are_not_kept() {
    // This aniso loop gives no displacement parameters the importer reads,
    // so the atoms stay isotropic and no aniso loop is written: its
    // unrecognized column must not be kept either, empty or not (as for
    // COD 4319054 while β was not read).
    let report = cif::read_str(
        "data_x\n_space_group_name_H-M_alt 'P 1'\n\
         _cell_length_a 5\n_cell_length_b 5\n_cell_length_c 5\n\
         _cell_angle_alpha 90\n_cell_angle_beta 90\n_cell_angle_gamma 90\n\
         loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         Ag1 0 0 0\nO1 0.5 0.5 0.5\n\
         loop_\n_atom_site_aniso_label\n_atom_site_aniso_Note\n\
         Ag1 first\nO1 second\n",
    );
    let (block, structure) = report.structures().next().unwrap();
    common::round_trip(block, structure).unwrap_or_else(|m| panic!("{m:?}"));
}

#[test]
fn beta_tensors_are_written_back_as_u() {
    // β is read and converted to U (COD 4319054): written as U, it reads
    // back as the same displacement parameters.
    let report = cif::read_str(
        "data_x\n_space_group_name_H-M_alt 'P 1'\n\
         _cell_length_a 5\n_cell_length_b 6\n_cell_length_c 10\n\
         _cell_angle_alpha 90\n_cell_angle_beta 100\n_cell_angle_gamma 90\n\
         loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         Ag1 0 0 0\nO1 0.5 0.5 0.5\n\
         loop_\n_atom_site_aniso_label\n\
         _atom_site_aniso_beta_11\n_atom_site_aniso_beta_22\n_atom_site_aniso_beta_33\n\
         _atom_site_aniso_beta_12\n_atom_site_aniso_beta_13\n_atom_site_aniso_beta_23\n\
         Ag1 0.007(1) 0.019(1) 0.0023(2) 0.0 0.0 -0.0006(7)\n\
         O1 0.007(1) 0.013(1) 0.0009(3) 0.0 0.0 0.0\n",
    );
    let (block, structure) = report.structures().next().unwrap();
    let written = common::round_trip(block, structure).unwrap_or_else(|m| panic!("{m:?}"));
    assert!(
        written.text.contains("_atom_site_aniso_U_11"),
        "{}",
        written.text
    );
    assert!(
        !written.text.contains("_atom_site_aniso_beta"),
        "{}",
        written.text
    );
}

#[test]
fn values_that_round_to_zero_lose_their_sign() {
    // B12 = -0.03(47) is U12 = -0.00038 with an s.u. of 0.006: written at
    // three decimals it is `-0.000(6)`, which reads back as zero and is
    // written `0.000(6)` the next time (COD 2002519).
    let report = cif::read_str(
        "data_x\n_space_group_name_H-M_alt 'P 1'\n\
         _cell_length_a 5\n_cell_length_b 5\n_cell_length_c 5\n\
         _cell_angle_alpha 90\n_cell_angle_beta 90\n_cell_angle_gamma 90\n\
         loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         O1 0.1 0.2 0.3\n\
         loop_\n_atom_site_aniso_label\n\
         _atom_site_aniso_B_11\n_atom_site_aniso_B_22\n_atom_site_aniso_B_33\n\
         _atom_site_aniso_B_12\n_atom_site_aniso_B_13\n_atom_site_aniso_B_23\n\
         O1 0.8(5) 1.6(3) 1.4(5) -0.03(47) -0.16(47) -0.32(47)\n",
    );
    let (block, structure) = report.structures().next().unwrap();
    let written = common::round_trip(block, structure).unwrap_or_else(|m| panic!("{m:?}"));
    assert!(!written.text.contains("-0.000("), "{}", written.text);
}

#[test]
fn kept_columns_follow_renamed_labels() {
    // SHELXL-97 reuses hydrogen labels; the importer renames the second
    // H1 to H1_2. Its unrecognized columns, in the atom site loop and the
    // aniso loop, must go with it rather than stay with the first H1.
    let text = "data_x\n\
                _space_group_name_H-M_alt 'P 1'\n\
                _cell_length_a 10\n_cell_length_b 10\n_cell_length_c 10\n\
                _cell_angle_alpha 90\n_cell_angle_beta 90\n_cell_angle_gamma 90\n\
                loop_\n_atom_site_label\n_atom_site_type_symbol\n\
                _atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
                _atom_site_calc_flag\n\
                H1 H 0.1 0.1 0.1 d\n\
                H1 H 0.3 0.3 0.3 calc\n\
                C1 C 0.5 0.5 0.5 d\n\
                loop_\n_atom_site_aniso_label\n\
                _atom_site_aniso_U_11\n_atom_site_aniso_U_22\n_atom_site_aniso_U_33\n\
                _atom_site_aniso_U_12\n_atom_site_aniso_U_13\n_atom_site_aniso_U_23\n\
                _atom_site_aniso_Note\n\
                H1 0.01 0.01 0.01 0 0 0 first\n\
                H1 0.02 0.02 0.02 0 0 0 second\n";
    let report = cif::read_str(text);
    let (block, structure) = report.structures().next().unwrap();
    let labels: Vec<&str> = structure
        .sites
        .iter()
        .map(|site| site.occupants[0].label.as_str())
        .collect();
    assert_eq!(labels, ["H1", "H1_2", "C1"]);
    let written = common::round_trip(block, structure).unwrap_or_else(|m| panic!("{m:?}"));
    let row = |label: &str| {
        written
            .text
            .lines()
            .filter(|line| line.split_whitespace().next() == Some(label))
            .collect::<Vec<_>>()
    };
    // The atom site row, then the aniso row.
    let h1 = row("H1");
    assert!(h1[0].ends_with(" d"), "{h1:?}");
    assert!(h1[1].ends_with(" first"), "{h1:?}");
    let h1_2 = row("H1_2");
    assert!(h1_2[0].ends_with(" calc"), "{h1_2:?}");
    assert!(h1_2[1].ends_with(" second"), "{h1_2:?}");
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

#[test]
fn coordinates_that_round_to_one_are_written_as_zero() {
    // Inside the cell, but 1 at the ten decimals written: the atom is
    // written at 0, which is the same place, so that every coordinate
    // stays in [0, 1) (design §9.3).
    let report = cif::read_str(
        "data_x\n_space_group_name_H-M_alt 'P 1'\n\
         _cell_length_a 4\n_cell_length_b 4\n_cell_length_c 4\n\
         loop_\n_atom_site_label\n_atom_site_type_symbol\n\
         _atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         Na1 Na 0.99999999999 0.5 0.999999999999999\n",
    );
    let structure = report.structures().next().unwrap().1;
    let poscar = poscar::write(structure, "").unwrap();
    assert_eq!(
        poscar.lines().last(),
        Some("    0.0000000000    0.5000000000    0.0000000000")
    );
    let xyz = xyz::write(structure).unwrap();
    assert_eq!(
        xyz.lines().last(),
        Some("Na    0.0000000000    2.0000000000    0.0000000000")
    );
}
