//! Symmetrizing imported structures (crystallography design §7.10): the
//! P1 files among the fixtures get their symmetry back, and the result
//! survives a CIF round trip, a left-handed lattice included.

use std::path::Path;

use mw_crystal::{Derived, DetectSymmetry, Structure, Symmetrization, SymmetryAnalysis};
use mw_crystal_io::{ImportReport, cif, poscar};

fn read(name: &str) -> ImportReport {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    if name.ends_with(".cif") {
        cif::read(&bytes)
    } else {
        poscar::read(&bytes)
    }
}

fn structure(name: &str) -> Structure {
    let report = read(name);
    let [block] = &report.blocks[..] else {
        panic!("{name}: one block");
    };
    block.structure.clone().expect("a structure")
}

fn symmetrize(structure: &Structure, standardize: bool) -> Symmetrization {
    DetectSymmetry {
        symprec: structure.tolerances.symprec,
        standardize,
    }
    .run(structure)
    .unwrap()
}

fn number(structure: &Structure) -> u16 {
    Derived::compute(structure)
        .symmetry
        .space_group
        .map_or(0, |sg| sg.number)
}

/// Writes the structure as CIF and reads it back.
fn through_cif(structure: &Structure) -> Structure {
    let written = cif::write(structure, &cif::WriteOptions::new("symmetrized"));
    let report = cif::read_str(&written.text);
    let [block] = &report.blocks[..] else {
        panic!("one block");
    };
    block.structure.clone().expect("a structure")
}

fn atoms_in_cell(structure: &Structure) -> usize {
    Derived::compute(structure).atoms_in_cell()
}

#[test]
fn halite_written_in_p1() {
    let halite = structure("halite_p1.cif");
    let result = symmetrize(&halite, false);
    let s = &result.structure;
    assert_eq!(number(s), 225);
    assert_eq!(s.symmetry.operations().len(), 192);
    let labels: Vec<&str> = s.sites.iter().map(|site| site.label()).collect();
    assert_eq!(labels.len(), 2);
    assert_eq!(atoms_in_cell(s), 8);
    let again = through_cif(s);
    assert_eq!(number(&again), 225);
    assert_eq!(atoms_in_cell(&again), 8);
}

#[test]
fn primitive_and_conventional_poscars() {
    // GaAs in its primitive cell, given in Cartesian coordinates.
    let gaas = structure("poscar/gaas_primitive_cartesian.vasp");
    let kept = symmetrize(&gaas, false);
    assert_eq!(kept.detected_number, 216);
    assert_eq!(kept.structure.symmetry.operations().len(), 24);
    let standard = symmetrize(&gaas, true);
    let s = &standard.structure;
    assert_eq!(number(s), 216);
    assert_eq!(atoms_in_cell(s), 8);
    assert!((s.lattice.params().a.value() - 5.653).abs() < 1e-9);
    assert!(standard.cell_changed);
    assert_eq!(number(&through_cif(s)), 216);

    // Silicon in its conventional cell: Fd-3m, origin choice 2 or 1
    // depending on where the file puts the origin; here an atom is at the
    // origin, which is origin choice 1.
    let si = structure("poscar/si_diamond.vasp");
    let result = symmetrize(&si, false);
    assert_eq!(number(&result.structure), 227);
    assert_eq!(result.origin_shift, 0.0);
    assert_eq!(result.structure.sites.len(), 1);
    assert_eq!(result.structure.sites[0].position, [0.0; 3]);
}

#[test]
fn a_left_handed_lattice() {
    let zno = structure("poscar/zno_left_handed.vasp");
    assert!(zno.lattice.is_left_handed());
    let kept = symmetrize(&zno, false);
    let s = &kept.structure;
    assert!(s.lattice.is_left_handed());
    assert_eq!(number(s), 186);
    assert_eq!(s.sites.len(), 2);
    // CIF holds it on −a, −b, −c, with the operations of P6₃mc.
    let again = through_cif(s);
    assert!(!again.lattice.is_left_handed());
    assert_eq!(number(&again), 186);
    let analysis = SymmetryAnalysis::compute(&again, &mw_crystal::orbits(&again));
    assert_eq!(analysis.space_group.map(|sg| sg.number), Some(186));
    assert_eq!(atoms_in_cell(&again), 4);
    // Standardized, it is right-handed already.
    let standard = symmetrize(&zno, true);
    assert!(!standard.structure.lattice.is_left_handed());
    assert!(standard.orientation_changed);
    assert_eq!(number(&standard.structure), 186);
}
