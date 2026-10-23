//! Projects of older format versions (design §4): each version's test
//! project under `tests/fixtures/projects/v<n>/` opens with the current
//! code, holds what it held, and its first save backs up the old files and
//! writes the current version.

use std::fs;
use std::path::{Path, PathBuf};

use mw_project_file::{Manifest, ProjectDir, Save, StructureFile, VERSION, read_manifest, save};

fn fixture(version: u32, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/projects")
        .join(format!("v{version}"))
        .join(name)
}

fn sample(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("samples")
        .join(name)
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            fs::copy(&path, &target).unwrap();
        }
    }
}

/// The manifest and every structure of a project.
fn content(dir: &ProjectDir, manifest: &Manifest) -> Vec<StructureFile> {
    manifest
        .entries
        .iter()
        .map(|entry| dir.read_structure(entry.id).unwrap())
        .collect()
}

#[test]
fn version_1_projects_open_as_they_were() {
    // The v1 test project is the TiO₂ sample as version 1 wrote it; the
    // sample in `samples/` is the same project in the current version.
    let (old, old_manifest) = read_manifest(&fixture(1, "tio2.mwproj")).unwrap();
    assert!(old.is_migrated());
    let (current, manifest) = read_manifest(&sample("tio2.mwproj")).unwrap();
    assert!(!current.is_migrated());
    assert_eq!(old_manifest, manifest);
    let structures = content(&old, &old_manifest);
    assert_eq!(structures, content(&current, &manifest));
    // Version 1 had no orientations: every lattice is standard.
    assert!(
        structures
            .iter()
            .all(|file| *file.structure.lattice.orientation() == Default::default())
    );
    for entry in &old_manifest.entries {
        if let Some(import) = entry.provenance.import() {
            let bytes = old.read_source(import).unwrap();
            let block = import.read_block(&bytes).unwrap();
            assert_eq!(
                mw_project_file::structure_sha256(&block.structure.unwrap()),
                import.structure_sha256
            );
        }
    }
    assert_eq!(old.read_view(), current.read_view());
}

#[test]
fn saving_a_version_1_project_backs_it_up_and_writes_the_current_version() {
    let temp = tempfile::tempdir().unwrap();
    let original = fixture(1, "tio2.mwproj");
    let path = temp.path().join("tio2.mwproj");
    copy_dir(&original, &path);
    let (dir, manifest) = read_manifest(&path).unwrap();
    let before = content(&dir, &manifest);
    let saved = save(
        &path,
        &Save {
            manifest: manifest.clone(),
            structures: Vec::new(),
            sources: Vec::new(),
            from: Some(dir),
            view: None,
        },
    )
    .unwrap();
    assert!(!saved.is_migrated());

    // The replaced JSON files are in backup/format-v1/, byte for byte.
    let backup = path.join("backup/format-v1");
    let same = |relative: &str| {
        assert_eq!(
            fs::read(backup.join(relative)).unwrap(),
            fs::read(original.join(relative)).unwrap(),
            "{relative}"
        );
    };
    same("project.json");
    for entry in &manifest.entries {
        same(&format!("structures/{}.json", entry.id));
    }

    // The project now reads as the current version, with the same content,
    // in the same bytes as the current sample.
    let manifest_text = fs::read_to_string(path.join("project.json")).unwrap();
    assert!(manifest_text.contains(&format!("\"version\": {VERSION},")));
    let (dir, read) = read_manifest(&path).unwrap();
    assert!(!dir.is_migrated());
    assert_eq!(read, manifest);
    assert_eq!(content(&dir, &read), before);
    for relative in ["project.json"].into_iter().map(String::from).chain(
        manifest
            .entries
            .iter()
            .map(|e| format!("structures/{}.json", e.id)),
    ) {
        assert_eq!(
            fs::read(path.join(&relative)).unwrap(),
            fs::read(sample("tio2.mwproj").join(&relative)).unwrap(),
            "{relative}"
        );
    }
}
