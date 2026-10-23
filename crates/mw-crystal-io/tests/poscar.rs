//! POSCAR import (design §9.3): the hand-written files under
//! `fixtures/poscar/`, errors and warnings at their lines, and round trips
//! through the POSCAR writer (export → import → the same atoms, and the
//! same text when written again).

use std::path::Path;

use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{
    Derived, Lattice, Orientation, Rotation, Structure, UNKNOWN_ELEMENT, make_supercell,
};
use mw_crystal_io::{ImportReport, Severity, cif, poscar};

fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/poscar")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn fixture(name: &str) -> ImportReport {
    poscar::read(&fixture_bytes(name))
}

/// The structure of a report that has one, with its diagnostics as
/// (code, line).
fn structure(report: &ImportReport) -> &Structure {
    let [block] = &report.blocks[..] else {
        panic!("one block");
    };
    block
        .structure
        .as_ref()
        .unwrap_or_else(|| panic!("no structure: {:?}", block.diagnostics))
}

fn codes(report: &ImportReport) -> Vec<(&'static str, Option<u32>)> {
    report
        .all_diagnostics()
        .map(|d| (d.code(), d.line()))
        .collect()
}

fn labels(structure: &Structure) -> Vec<&str> {
    structure.sites.iter().map(|s| s.label()).collect()
}

fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

#[test]
fn vasp5_with_direct_coordinates() {
    let report = fixture("si_diamond.vasp");
    assert_eq!(codes(&report), []);
    let si = structure(&report);
    assert_eq!(
        report.blocks[0].name,
        "Si, diamond (F d -3 m), conventional cell; hand-written test fixture for Materials Workbench, a = 5.431 A (literature value)"
    );
    assert!(si.symmetry.is_p1());
    assert_eq!(si.space_group, Default::default());
    assert_eq!(
        labels(si),
        ["Si1", "Si2", "Si3", "Si4", "Si5", "Si6", "Si7", "Si8"]
    );
    let params = si.lattice.params();
    assert_eq!(
        [params.a, params.b, params.c].map(|m| m.value()),
        [5.431; 3]
    );
    assert_eq!(
        [params.alpha, params.beta, params.gamma].map(|m| m.value()),
        [90.0; 3]
    );
    assert_eq!(si.lattice.orientation(), &Orientation::Standard);
    assert_eq!(si.sites[4].position, [0.25, 0.25, 0.25]);
    assert!(
        si.sites
            .iter()
            .all(|s| s.occupants[0].occupancy.value() == 1.0)
    );
    assert_eq!(si.validate(), Ok(()));
    let detected = Derived::compute(si).symmetry.detected.expect("searched");
    assert_eq!(detected.number, 227);
}

#[test]
fn cartesian_coordinates_and_turned_vectors() {
    let report = fixture("gaas_primitive_cartesian.vasp");
    assert_eq!(codes(&report), []);
    let gaas = structure(&report);
    assert_eq!(labels(gaas), ["Ga1", "As1"]);
    let params = gaas.lattice.params();
    let a = 5.653 / 2f64.sqrt();
    assert!(close(params.a.value(), a, 1e-12));
    assert!(close(params.c.value(), a, 1e-12));
    // The fcc primitive angles come out exactly 60°.
    assert_eq!(params.alpha.value(), 60.0);
    assert_eq!(params.gamma.value(), 60.0);
    // a = (0, ½, ½)·5.653 does not lie along x: the orientation is kept.
    let rotation = gaas.lattice.orientation().rotation().expect("explicit");
    assert!(close(rotation.determinant(), 1.0, 1e-12));
    let matrix = gaas.lattice.matrix();
    let expected =
        [[0.0, 0.5, 0.5], [0.5, 0.0, 0.5], [0.5, 0.5, 0.0]].map(|v| v.map(|x| x * 5.653));
    for (got, want) in matrix.iter().flatten().zip(expected.iter().flatten()) {
        assert!(close(*got, *want, 1e-12), "{matrix:?}");
    }
    // Cartesian (¼, ¼, ¼)·5.653 is fractional (¼, ¼, ¼).
    for (x, want) in gaas.sites[1].position.iter().zip([0.25; 3]) {
        assert!(close(*x, want, 1e-12), "{:?}", gaas.sites[1].position);
    }
    let r = gaas.lattice.to_cartesian(gaas.sites[1].position);
    assert!(r.iter().all(|v| close(*v, 0.25 * 5.653, 1e-12)), "{r:?}");
    let detected = Derived::compute(gaas).symmetry.detected.expect("searched");
    assert_eq!(detected.number, 216);
}

#[test]
fn a_negative_scaling_factor_is_the_volume() {
    let report = fixture("rutile_volume.vasp");
    assert_eq!(codes(&report), []);
    let rutile = structure(&report);
    let params = rutile.lattice.params();
    assert!(close(params.volume(), 62.449209724, 1e-9));
    assert!(close(params.a.value(), 4.594, 1e-8), "{}", params.a.value());
    assert!(close(params.c.value(), 2.959, 1e-8), "{}", params.c.value());
    assert_eq!(labels(rutile), ["Ti1", "Ti2", "O1", "O2", "O3", "O4"]);
}

#[test]
fn contcar_with_selective_dynamics_and_velocities() {
    let report = fixture("CONTCAR");
    // The flags are dropped with a warning, the velocities with a note.
    assert_eq!(
        codes(&report),
        [
            ("poscar.selective_dynamics_ignored", Some(8)),
            ("poscar.trailing_content", Some(17)),
        ]
    );
    let severities: Vec<Severity> = report.all_diagnostics().map(|d| d.severity()).collect();
    assert_eq!(severities, [Severity::Warning, Severity::Info]);
    let rutile = structure(&report);
    // POTCAR suffixes name the element.
    assert_eq!(labels(rutile), ["Ti1", "Ti2", "O1", "O2", "O3", "O4"]);
    assert_eq!(rutile.sites[0].occupants[0].species.symbol, "Ti");
    assert_eq!(rutile.sites[2].position, [0.3048, 0.3048, 0.0]);
    // The same atoms as the rutile CIF, in P1.
    let cif = cif::read(
        &std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rutile.cif"))
            .unwrap(),
    );
    let reference = cif.structures().next().unwrap().1;
    assert_eq!(rutile.lattice, reference.lattice);
    let detected = Derived::compute(rutile)
        .symmetry
        .detected
        .expect("searched");
    assert_eq!(detected.number, 136);
}

#[test]
fn vasp4_files_need_their_elements() {
    let bytes = fixture_bytes("gaas_vasp4.vasp");
    let report = poscar::read(&bytes);
    assert_eq!(codes(&report), [("poscar.missing_elements", Some(6))]);
    assert!(report.blocks[0].structure.is_none());
    assert!(matches!(
        report.all_diagnostics().next().unwrap().kind(),
        mw_crystal_io::DiagnosticKind::MissingElements { species: 2 }
    ));

    let report = poscar::read_with_elements(&bytes, &["Ga", "As"]);
    assert_eq!(codes(&report), []);
    assert_eq!(labels(structure(&report)), ["Ga1", "As1"]);
    let report = poscar::read_with_elements(&bytes, &["Ga"]);
    assert_eq!(codes(&report), [("poscar.species_count_mismatch", Some(6))]);
    // A file with an element line keeps its own.
    let report = poscar::read_with_elements(&fixture_bytes("si_diamond.vasp"), &["C"]);
    assert_eq!(structure(&report).sites[0].element(), "Si");
}

#[test]
fn left_handed_vectors_are_kept_and_noted() {
    let report = fixture("zno_left_handed.vasp");
    assert_eq!(codes(&report), [("poscar.left_handed", Some(3))]);
    assert_eq!(report.blocks[0].diagnostics[0].severity(), Severity::Info);
    let zno = structure(&report);
    assert!(zno.lattice.is_left_handed());
    let params = zno.lattice.params();
    assert!(close(params.a.value(), 3.2498, 1e-9));
    assert_eq!(params.gamma.value(), 120.0);
    let matrix = zno.lattice.matrix();
    let expected = [
        [-1.6249, 2.8144093572, 0.0],
        [3.2498, 0.0, 0.0],
        [0.0, 0.0, 5.2066],
    ];
    for (got, want) in matrix.iter().flatten().zip(expected.iter().flatten()) {
        assert!(close(*got, *want, 1e-9), "{matrix:?}");
    }
    assert_eq!(labels(zno), ["Zn1", "Zn2", "O1", "O2"]);
    // Wurtzite either way: it has mirror planes.
    let detected = Derived::compute(zno).symmetry.detected.expect("searched");
    assert_eq!(detected.number, 186);
}

/// A POSCAR with the given scaling line, lattice, species, counts, mode
/// and coordinate lines.
fn poscar_text(scale: &str, lattice: [&str; 3], head: &[&str], atoms: &[&str]) -> String {
    let mut lines = vec!["test", scale];
    lines.extend(lattice);
    lines.extend(head);
    lines.extend(atoms);
    lines.join("\n") + "\n"
}

const CUBIC: [&str; 3] = ["4 0 0", "0 4 0", "0 0 4"];

fn read_codes(text: &str) -> Vec<(&'static str, Option<u32>)> {
    codes(&poscar::read_str(text))
}

#[test]
fn errors_and_warnings_at_their_lines() {
    let na_cl = ["Na Cl", "1 1", "Direct"];
    let atoms = ["0 0 0", "0.5 0.5 0.5"];
    // A complete file reads without findings.
    assert_eq!(read_codes(&poscar_text("1", CUBIC, &na_cl, &atoms)), []);

    assert_eq!(read_codes(""), [("poscar.truncated", None)]);
    assert_eq!(read_codes("test\n1\n4 0 0\n"), [("poscar.truncated", None)]);
    for scale in ["0", "x", "1 2", "-1 1 1"] {
        assert_eq!(
            read_codes(&poscar_text(scale, CUBIC, &na_cl, &atoms)),
            [("poscar.invalid_scale", Some(2))],
            "{scale}"
        );
    }
    assert_eq!(
        read_codes(&poscar_text("1 1 2", CUBIC, &na_cl, &atoms)),
        [("poscar.unequal_scale_factors", Some(2))]
    );
    // Three equal factors are one; text after the number is a comment.
    for scale in ["2 2 2", "2.0 ! lattice constant", "2D0"] {
        let report = poscar::read_str(&poscar_text(scale, CUBIC, &na_cl, &atoms));
        assert_eq!(
            structure(&report).lattice.params().a.value(),
            8.0,
            "{scale}"
        );
    }
    assert_eq!(
        read_codes(&poscar_text("1", ["4 0 0", "0 4", "0 0 4"], &na_cl, &atoms)),
        [("poscar.invalid_lattice_vector", Some(4))]
    );
    assert_eq!(
        read_codes(&poscar_text(
            "1",
            ["4 0 0", "8 0 0", "0 0 4"],
            &na_cl,
            &atoms
        )),
        [("poscar.degenerate_lattice", Some(3))]
    );
    assert_eq!(
        read_codes(&poscar_text(
            "-10",
            ["4 0 0", "8 0 0", "0 0 4"],
            &na_cl,
            &atoms
        )),
        [("poscar.degenerate_lattice", Some(3))]
    );
    assert_eq!(
        read_codes(&poscar_text(
            "1",
            CUBIC,
            &["Na Cl", "one", "Direct"],
            &atoms
        )),
        [("poscar.invalid_counts", Some(7))]
    );
    // Counts too large to add up, or for the lines the file has.
    assert_eq!(
        read_codes(&poscar_text(
            "1",
            CUBIC,
            &["Na Cl", "18446744073709551615 1", "Direct"],
            &atoms
        )),
        [("poscar.invalid_counts", Some(7))]
    );
    assert_eq!(
        read_codes(&poscar_text(
            "1",
            CUBIC,
            &["Na Cl", "1000000000 1", "Direct"],
            &atoms
        )),
        // At the last line, where the file ends.
        [("poscar.missing_coordinates", Some(10))]
    );
    assert_eq!(
        read_codes(&poscar_text(
            "1",
            CUBIC,
            &["Na Cl", "1 1 1", "Direct"],
            &atoms
        )),
        [("poscar.species_count_mismatch", Some(7))]
    );
    assert_eq!(
        read_codes(&poscar_text("1", CUBIC, &["Na Cl", "0 0", "Direct"], &[])),
        [("atom.missing_sites", Some(7))]
    );
    // Coordinates where the coordinate type should be.
    assert_eq!(
        read_codes(&poscar_text("1", CUBIC, &["Na Cl", "1 1"], &atoms)),
        [("poscar.invalid_coordinate_mode", Some(8))]
    );
    assert_eq!(
        read_codes(&poscar_text("1", CUBIC, &na_cl, &["0 0", "0.5 0.5 x"])),
        [
            ("poscar.invalid_coordinates", Some(9)),
            ("poscar.invalid_coordinates", Some(10)),
        ]
    );
    assert_eq!(
        read_codes(&poscar_text("1", CUBIC, &na_cl, &["0 0 0"])),
        [("poscar.missing_coordinates", Some(9))]
    );
    // An unknown name stays as X; atoms on top of each other are reported.
    let report = poscar::read_str(&poscar_text(
        "1",
        CUBIC,
        &["Na Qq", "1 1", "Direct"],
        &["0 0 0", "0.01 0 0"],
    ));
    assert_eq!(
        codes(&report),
        [
            ("atom.unknown_element", Some(6)),
            ("atom.short_contact", Some(10)),
        ]
    );
    let read = structure(&report);
    assert_eq!(labels(read), ["Na1", "Qq1"]);
    assert_eq!(read.sites[1].element(), UNKNOWN_ELEMENT);
    assert_eq!(read.sites[1].occupants[0].species.symbol, "Qq");
    // An element listed twice keeps counting its labels.
    let report = poscar::read_str(&poscar_text(
        "1",
        CUBIC,
        &["Fe O Fe", "1 1 1", "Direct"],
        &["0 0 0", "0.5 0.5 0.5", "0.5 0 0"],
    ));
    assert_eq!(labels(structure(&report)), ["Fe1", "O1", "Fe2"]);
    // Bytes that are not UTF-8 are replaced and reported.
    let mut bytes = poscar_text("1", CUBIC, &na_cl, &atoms).into_bytes();
    bytes[2] = 0xff;
    let report = poscar::read(&bytes);
    assert_eq!(codes(&report), [("syntax.not_utf8", None)]);
}

/// Structures the POSCAR writer accepts: every fixture structure, the
/// built-in samples, supercells (one not diagonal, so explicitly oriented)
/// and turned and left-handed copies of rutile.
fn exportable() -> Vec<(String, Structure)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "cif"))
        .collect();
    names.sort();
    let mut structures = Vec::new();
    for path in names {
        let report = cif::read(&std::fs::read(&path).unwrap());
        for (block, structure) in report.structures() {
            let name = format!("{}/{}", path.file_name().unwrap().display(), block.name);
            structures.push((name, structure.clone()));
        }
    }
    for key in [
        SampleKey::Rutile,
        SampleKey::RutileSupercell,
        SampleKey::StrontiumTitanate,
    ] {
        structures.push((format!("{key:?}"), sample::load(key).structure));
    }
    let rutile = sample::load(SampleKey::Rutile).structure;
    let srtio3 = sample::load(SampleKey::StrontiumTitanate).structure;
    structures.push((
        "srtio3 √2×√2×1".into(),
        make_supercell(&srtio3, [[1, 1, 0], [-1, 1, 0], [0, 0, 1]]).unwrap(),
    ));
    let (s, c) = 0.3f64.sin_cos();
    for (name, rows) in [
        (
            "turned rutile",
            [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
        ),
        (
            "left-handed rutile",
            [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, -1.0]],
        ),
    ] {
        let mut turned = rutile.clone();
        turned.lattice = Lattice::with_orientation(
            *rutile.lattice.params(),
            Orientation::new(Rotation::new(rows).unwrap()),
        );
        structures.push((name.into(), turned));
    }
    structures.retain(|(_, structure)| poscar::check(structure).is_ok());
    structures
}

/// Whether `read` holds the atoms of `original`'s unit cell: the same
/// elements at the same Cartesian positions, up to lattice translations,
/// on the same lattice vectors.
fn same_atoms(original: &Structure, read: &Structure) -> Result<(), String> {
    let mut atoms: Vec<(&str, [f64; 3])> = Vec::new();
    for (site, orbit) in original.sites.iter().zip(mw_crystal::orbits(original)) {
        for position in orbit.positions {
            atoms.push((site.element(), original.lattice.to_cartesian(position)));
        }
    }
    if atoms.len() != read.sites.len() {
        return Err(format!("{} atoms, read {}", atoms.len(), read.sites.len()));
    }
    let (a, b) = (original.lattice.matrix(), read.lattice.matrix());
    if !a
        .iter()
        .flatten()
        .zip(b.iter().flatten())
        .all(|(x, y)| close(*x, *y, 1e-9))
    {
        return Err(format!("lattice {a:?}, read {b:?}"));
    }
    if original.lattice.is_left_handed() != read.lattice.is_left_handed() {
        return Err("handedness".into());
    }
    for site in &read.sites {
        let r = read.lattice.to_cartesian(site.position);
        let found = atoms.iter().position(|(element, a)| {
            let d = original
                .lattice
                .to_fractional([0, 1, 2].map(|i| r[i] - a[i]));
            *element == site.element() && d.iter().all(|v| close(*v, v.round(), 1e-9))
        });
        match found {
            Some(i) => {
                atoms.swap_remove(i);
            }
            None => {
                return Err(format!(
                    "{} at {r:?} is no atom of the original",
                    site.label()
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn exported_structures_read_back() {
    let structures = exportable();
    // 15 fixture structures without partial occupancy, 3 samples, and 3
    // with an explicit orientation, one of them left-handed. Update when
    // adding a fixture.
    assert_eq!(structures.len(), 21);
    for (name, structure) in &structures {
        let text = poscar::write(structure, name).unwrap();
        let report = poscar::read_str(&text);
        let findings: Vec<_> = report
            .all_diagnostics()
            .filter(|d| d.code() != "poscar.left_handed")
            .map(|d| d.code())
            .collect();
        assert_eq!(findings, Vec::<&str>::new(), "{name}");
        let read = structure_of(&report, name);
        if let Err(mismatch) = same_atoms(structure, read) {
            panic!("{name}: {mismatch}");
        }
        assert_eq!(
            read.lattice.orientation() == &Orientation::Standard,
            structure.lattice.orientation() == &Orientation::Standard,
            "{name}"
        );
        assert_eq!(report.blocks[0].name, name.as_str());
        // Written again, the file is the same.
        assert_eq!(poscar::write(read, name).unwrap(), text, "{name}");
    }
}

fn structure_of<'a>(report: &'a ImportReport, name: &str) -> &'a Structure {
    report.blocks[0]
        .structure
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: {:?}", report.blocks[0].diagnostics))
}

#[test]
fn files_cut_anywhere_read_without_panicking() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/poscar");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for end in (0..=text.len()).filter(|&i| text.is_char_boundary(i)) {
            let report = poscar::read_str(&text[..end]);
            assert_eq!(report.blocks.len(), 1, "{}", path.display());
        }
    }
}
