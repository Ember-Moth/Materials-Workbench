//! The sample projects shipped with the application (design §11), built
//! from the import fixtures of `mw-crystal-io` the way the interface builds
//! projects: by importing files and deriving structures.
//!
//! `samples/` holds the result. Regenerate it after changing the format or
//! the fixtures:
//!
//! ```bash
//! cargo test -p mw-project-file --test samples -- --ignored
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mw_crystal::sample::{self, SampleKey};
use mw_project_file::{
    Derived, Entry, EntryId, EntryKind, Import, Manifest, Operation, ProjectId, Provenance, Save,
    Sha256, Source, SourceFormat, StructureFile, Timestamp, ViewState, read_manifest, samples,
    save, structure_sha256,
};

fn samples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("samples")
}

/// 2026-10-22T00:00:00Z, so that regenerating gives the same files.
const MADE: u64 = 1_792_627_200;

struct Imported {
    entry: Entry,
    file: StructureFile,
    source: Source,
}

fn import(file_name: &str, id: EntryId, name: &str) -> Imported {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mw-crystal-io/tests/fixtures")
        .join(file_name);
    let bytes: Arc<[u8]> = fs::read(path).unwrap().into();
    let report = mw_crystal_io::cif::read(&bytes);
    let (block, structure) = report.structures().next().expect("a structure");
    let sha = Sha256::of(&bytes);
    let import = Import::new(
        SourceFormat::Cif,
        file_name,
        None,
        sha.clone(),
        block.name.clone(),
        Timestamp::from_unix(MADE),
        structure_sha256(structure),
    );
    Imported {
        entry: Entry::new(
            id,
            EntryKind::CrystalStructure,
            name,
            Provenance::Import(import),
        ),
        file: StructureFile::new(structure.clone(), Vec::new(), Vec::new()),
        source: Source {
            sha256: sha,
            format: SourceFormat::Cif,
            bytes,
        },
    }
}

fn project(
    id: u128,
    name: &str,
    imported: Vec<Imported>,
    derived: Vec<(Entry, StructureFile)>,
) -> Save {
    let mut manifest = Manifest::new(name);
    manifest.id = ProjectId::from_u128(id);
    let open = imported.first().map(|i| i.entry.id);
    let mut structures = Vec::new();
    let mut sources = Vec::new();
    for item in imported {
        manifest.entries.push(item.entry.clone());
        structures.push((item.entry.id, item.file));
        sources.push(item.source);
    }
    for (entry, file) in derived {
        manifest.entries.push(entry.clone());
        structures.push((entry.id, file));
    }
    let mut view = ViewState::default();
    view.open = open;
    Save {
        manifest,
        structures,
        sources,
        from: None,
        view: Some(view),
    }
}

/// Writes the sample projects into `dir`.
fn build(dir: &Path) {
    let rutile = import(
        "rutile.cif",
        EntryId::from_u128(0x6d2a_51c0_93e4_4b7a_8f10_2c4e_5a6b_7c01),
        "金红石",
    );
    let supercell = sample::load(SampleKey::RutileSupercell).structure;
    let derived = Derived::new(
        rutile.entry.id,
        rutile.entry.name.clone(),
        structure_sha256(&rutile.file.structure),
        Operation::MakeSupercell {
            matrix: [[2, 0, 0], [0, 2, 0], [0, 0, 2]],
        },
        Timestamp::from_unix(MADE),
    );
    let supercell = (
        Entry::new(
            EntryId::from_u128(0x6d2a_51c0_93e4_4b7a_8f10_2c4e_5a6b_7c02),
            EntryKind::CrystalStructure,
            "金红石 2×2×2 超胞",
            Provenance::Derived(derived),
        ),
        StructureFile::new(supercell, Vec::new(), Vec::new()),
    );
    let tio2 = project(
        0x6d2a_51c0_93e4_4b7a_8f10_2c4e_5a6b_7c00,
        "TiO₂ 相研究",
        vec![rutile],
        vec![supercell],
    );
    let srtio3 = import(
        "SrTiO3.cif",
        EntryId::from_u128(0x6d2a_51c0_93e4_4b7a_8f10_2c4e_5a6b_7d01),
        "SrTiO₃",
    );
    let perovskite = project(
        0x6d2a_51c0_93e4_4b7a_8f10_2c4e_5a6b_7d00,
        "钙钛矿氧化物",
        vec![srtio3],
        Vec::new(),
    );
    for (name, project) in [("tio2", tio2), ("perovskite", perovskite)] {
        save(&dir.join(format!("{name}.mwproj")), &project).unwrap();
    }
}

/// Every file under `dir` with its bytes, by relative path.
fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "json" || e == "cif") {
                let relative = path.strip_prefix(dir).unwrap().to_string_lossy();
                files.push((relative.replace('\\', "/"), fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

#[test]
#[ignore = "writes samples/; run after changing the format or the fixtures"]
fn regenerate() {
    let dir = samples_dir();
    for project in samples::PROJECTS {
        let _ = fs::remove_dir_all(dir.join(project));
    }
    build(&dir);
}

#[test]
fn the_samples_are_up_to_date() {
    let temp = tempfile::tempdir().unwrap();
    build(temp.path());
    let built = files(temp.path());
    assert_eq!(
        files(&samples_dir()),
        built,
        "regenerate the samples: cargo test -p mw-project-file --test samples -- --ignored"
    );
    let embedded: Vec<(String, Vec<u8>)> = samples::FILES
        .iter()
        .map(|file| (file.path.to_string(), file.bytes.to_vec()))
        .collect();
    let mut embedded_sorted = embedded.clone();
    embedded_sorted.sort();
    assert_eq!(embedded_sorted, built, "list every file in src/samples.rs");
}

#[test]
fn installed_samples_open() {
    let temp = tempfile::tempdir().unwrap();
    let installed = samples::install(temp.path()).unwrap();
    assert_eq!(installed.len(), 2);
    let mut names = Vec::new();
    for path in &installed {
        let (dir, manifest) = read_manifest(path).unwrap();
        for entry in &manifest.entries {
            let file = dir.read_structure(entry.id).unwrap();
            assert!(!file.structure.sites.is_empty());
            if let Some(import) = entry.provenance.import() {
                let bytes = dir.read_source(import).unwrap();
                let block = import.read_block(&bytes).unwrap();
                assert!(block.diagnostics.is_empty(), "{:?}", block.diagnostics);
                assert_eq!(block.structure.as_ref(), Some(&file.structure));
            }
            names.push(entry.name.clone());
        }
        assert_eq!(
            dir.read_view().unwrap().open,
            manifest.entries.first().map(|e| e.id)
        );
    }
    assert_eq!(names, ["金红石", "金红石 2×2×2 超胞", "SrTiO₃"]);

    // Installing again keeps what is there, edits included.
    let manifest = installed[0].join("project.json");
    let edited = fs::read_to_string(&manifest)
        .unwrap()
        .replace("金红石", "Rutile");
    fs::write(&manifest, &edited).unwrap();
    assert_eq!(samples::install(temp.path()).unwrap(), installed);
    assert_eq!(fs::read_to_string(&manifest).unwrap(), edited);
}
