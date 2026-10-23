//! Writes every structure of the test fixtures and the built-in samples as
//! POSCAR, with the CIF this crate writes for it, so that other programs
//! can read them back: the POSCAR check with pymatgen and ASE in
//! `docs/development.md` §2 (crystallography design §9.3).
//!
//! ```text
//! cargo run -p mw-crystal-io --example export_atoms -- <dir>
//! ```
//!
//! For each structure `<name>.vasp` and `<name>.cif`, and one line in
//! `manifest.tsv`: the name, the CIF it was imported from (or `-`), and
//! what the files should hold, worked out from the sites rather than from
//! the written text: the atoms in the cell, the elements in order with
//! their counts, and the cell parameters. Structures a list of atoms
//! cannot hold are listed as refused, with the reason.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{Structure, make_supercell};
use mw_crystal_io::{cif, poscar};

struct Entry {
    name: String,
    source: Option<PathBuf>,
    structure: Structure,
    /// Block name for the CIF.
    block: String,
}

fn fixtures(dir: &Path) -> Vec<Entry> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("the fixtures directory")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "cif"))
        .collect();
    paths.sort();
    let mut entries = Vec::new();
    for path in paths {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let report = cif::read(&std::fs::read(&path).expect("a fixture"));
        for (block, structure) in report.structures() {
            entries.push(Entry {
                name: format!("{stem}-{}", block.name),
                source: Some(path.clone()),
                structure: structure.clone(),
                block: block.name.clone(),
            });
        }
    }
    entries
}

/// The elements in the order they first occur among the sites, with the
/// number of atoms of each in the cell.
fn element_counts(structure: &Structure) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for (site, orbit) in structure.sites.iter().zip(mw_crystal::orbits(structure)) {
        let element = site.element().to_string();
        match counts.iter_mut().find(|(e, _)| *e == element) {
            Some((_, count)) => *count += orbit.positions.len(),
            None => counts.push((element, orbit.positions.len())),
        }
    }
    counts
}

fn main() {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: export_atoms <output directory>"),
    );
    std::fs::create_dir_all(&out).expect("create the output directory");
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");

    let mut entries = fixtures(&fixture_dir);
    for (key, name) in [
        (SampleKey::Rutile, "sample-rutile"),
        (SampleKey::RutileSupercell, "sample-rutile-supercell"),
        (SampleKey::StrontiumTitanate, "sample-srtio3"),
    ] {
        entries.push(Entry {
            name: name.into(),
            source: None,
            structure: sample::load(key).structure,
            block: name.replace('-', "_"),
        });
    }
    // Supercells as the interface makes them: the sample project's 2×2×2
    // rutile, and a cell that is not a multiple of the hexagonal one
    // (a' = a, b' = a + 2b), whose lattice the writer computes anew.
    let derived = [
        ("rutile-rutile", [[2, 0, 0], [0, 2, 0], [0, 0, 2]], "2x2x2"),
        (
            "zincite_legacy-zincite",
            [[1, 1, 0], [0, 2, 0], [0, 0, 1]],
            "orthohexagonal",
        ),
    ];
    for (from, matrix, suffix) in derived {
        let source = entries
            .iter()
            .find(|e| e.name == from)
            .unwrap_or_else(|| panic!("no fixture structure {from}"));
        let structure = make_supercell(&source.structure, matrix).expect("a supercell");
        let name = format!("supercell-{from}-{suffix}");
        entries.push(Entry {
            block: name.replace('-', "_"),
            name,
            source: None,
            structure,
        });
    }

    // Coordinates that round to 1 at the decimals written, and below 0
    // before wrapping: written as 0 and wrapped into the cell.
    let edge = cif::read_str(
        "data_edge\n_space_group_name_H-M_alt 'P 1'\n\
         _cell_length_a 4.1\n_cell_length_b 5.2\n_cell_length_c 6.3\n\
         _cell_angle_alpha 70\n_cell_angle_beta 80\n_cell_angle_gamma 100\n\
         loop_\n_atom_site_label\n_atom_site_type_symbol\n\
         _atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
         Na1 Na 0.99999999999 0.5 0.999999999999999\n\
         Cl1 Cl -0.25 -0.000000000001 0.5\n",
    );
    entries.push(Entry {
        name: "edge-near-one".into(),
        source: None,
        structure: edge.structures().next().expect("a structure").1.clone(),
        block: "edge".into(),
    });

    let mut manifest =
        String::from("name\tsource\tstatus\tatoms\telements\ta\tb\tc\talpha\tbeta\tgamma\n");
    let (mut written, mut refused) = (0, 0);
    for entry in &entries {
        let source = entry.source.as_ref().map_or_else(
            || "-".to_string(),
            |path| path.to_string_lossy().into_owned(),
        );
        let params = entry.structure.lattice.params();
        let cell = [
            params.a,
            params.b,
            params.c,
            params.alpha,
            params.beta,
            params.gamma,
        ]
        .map(|p| format!("{:.10}", p.value()))
        .join("\t");
        // The comment as the interface writes it: structure and project.
        let comment = format!("{} · 测试文件", entry.name);
        let status = match poscar::write(&entry.structure, &comment) {
            Ok(text) => {
                std::fs::write(out.join(format!("{}.vasp", entry.name)), text)
                    .expect("write the POSCAR");
                let options = cif::WriteOptions::new(&entry.block);
                let cif = cif::write(&entry.structure, &options).text;
                std::fs::write(out.join(format!("{}.cif", entry.name)), cif)
                    .expect("write the CIF");
                written += 1;
                "ok".to_string()
            }
            Err(error) => {
                refused += 1;
                format!("refused: {error}")
            }
        };
        let counts = element_counts(&entry.structure);
        let atoms: usize = counts.iter().map(|(_, n)| n).sum();
        let elements = counts
            .iter()
            .map(|(e, n)| format!("{e}:{n}"))
            .collect::<Vec<_>>()
            .join(",");
        let _ = writeln!(
            manifest,
            "{}\t{source}\t{status}\t{atoms}\t{elements}\t{cell}",
            entry.name
        );
    }
    std::fs::write(out.join("manifest.tsv"), manifest).expect("write the manifest");
    println!(
        "{} structures: {written} written to {}, {refused} refused",
        entries.len(),
        out.display()
    );
}
