//! Writing projects and reading them back (design §6, §9): every built-in
//! sample and every structure the import fixtures yield must come back
//! exactly, with measurements, the edit record, provenance and view state.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{
    AtomRef, Command, CrystalDocument, Edit, Lattice, Measurement, Orientation, Rotation, SiteId,
    Structure,
};
use mw_project_file::{
    Camera, Derived, DisorderGroup, DisplayModel, DisplayRange, Entry, EntryId, EntryKind,
    EntryView, Import, Manifest, Operation, PanelTab, Provenance, ReadError, Save, SaveError,
    Sha256, Source, SourceFormat, SphereCenter, StructureFile, Timestamp, VERSION, ViewState,
    read_manifest, save, structure_sha256, write_view,
};

/// The import fixtures of `mw-crystal-io`.
fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../mw-crystal-io/tests/fixtures")
}

/// A structure file with a measurement, a stale one and an edit record,
/// made the way the interface makes them: through a document.
fn document_file(structure: Structure) -> StructureFile {
    let mut document = CrystalDocument::new(structure);
    let one_cell = document
        .derived()
        .display(document.structure(), &Default::default());
    let atoms: Vec<_> = one_cell.atoms.iter().map(|a| a.atom).collect();
    let mut distinct = atoms.clone();
    distinct.dedup();
    if let [a, b, ..] = distinct[..] {
        document
            .execute(Command::AddMeasurement(Measurement::Distance([a, b])))
            .unwrap();
        // Removing the second atom's site leaves the measurement stale,
        // unless it is the only site.
        if document.structure().sites.len() > 1 {
            document.edit(Edit::RemoveSite { site: b.site }).unwrap();
        }
    }
    StructureFile::new(
        document.structure().clone(),
        document.annotations().to_vec(),
        document.history().cloned().collect(),
    )
}

struct Built {
    save: Save,
    files: Vec<(EntryId, StructureFile)>,
}

/// Rutile on left-handed basis vectors turned about z, as a POSCAR can give
/// them (crystallography design §4.2).
fn oriented_rutile() -> Structure {
    let mut rutile = sample::load(SampleKey::Rutile).structure;
    let (s, c) = 0.4f64.sin_cos();
    let rotation = Rotation::new([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, -1.0]]).unwrap();
    rutile.lattice =
        Lattice::with_orientation(*rutile.lattice.params(), Orientation::new(rotation));
    rutile
}

/// One project holding the samples, every structure of the fixtures and a
/// structure with an explicit orientation.
fn build() -> Built {
    let mut manifest = Manifest::new("测试项目");
    let mut files = Vec::new();
    let mut sources = Vec::new();
    let mut add = |manifest: &mut Manifest, name: String, provenance, structure| {
        let id = EntryId::new();
        manifest.entries.push(Entry::new(
            id,
            EntryKind::CrystalStructure,
            name,
            provenance,
        ));
        files.push((id, document_file(structure)));
        id
    };
    let now = Timestamp::now();
    let rutile = sample::load(SampleKey::Rutile).structure;
    let rutile_id = add(
        &mut manifest,
        "金红石".into(),
        Provenance::Created(mw_project_file::Created::new(now.clone())),
        rutile.clone(),
    );
    for key in [SampleKey::RutileSupercell, SampleKey::StrontiumTitanate] {
        let derived = Derived::new(
            rutile_id,
            "金红石",
            structure_sha256(&rutile),
            Operation::MakeSupercell {
                matrix: [[2, 0, 0], [0, 2, 0], [0, 0, 2]],
            },
            now.clone(),
        );
        add(
            &mut manifest,
            format!("{key:?}"),
            Provenance::Derived(derived),
            sample::load(key).structure,
        );
    }
    let mut names: Vec<_> = fs::read_dir(fixtures())
        .unwrap()
        .map(|f| f.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "cif"))
        .collect();
    names.sort();
    for path in names {
        let bytes: Arc<[u8]> = fs::read(&path).unwrap().into();
        let sha = Sha256::of(&bytes);
        let report = mw_crystal_io::cif::read(&bytes);
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        for (block, structure) in report.structures() {
            let import = Import::new(
                SourceFormat::Cif,
                file_name.clone(),
                Some(path.display().to_string()),
                sha.clone(),
                block.name.clone(),
                now.clone(),
                structure_sha256(structure),
            );
            add(
                &mut manifest,
                format!("{file_name} · {}", block.name),
                Provenance::Import(import),
                structure.clone(),
            );
            if !sources.iter().any(|s: &Source| s.sha256 == sha) {
                sources.push(Source {
                    sha256: sha.clone(),
                    format: SourceFormat::Cif,
                    bytes: bytes.clone(),
                });
            }
        }
    }
    // POSCAR files, some with an explicit orientation, one left-handed.
    let mut names: Vec<_> = fs::read_dir(fixtures().join("poscar"))
        .unwrap()
        .map(|f| f.unwrap().path())
        .filter(|p| p.extension().is_none_or(|e| e == "vasp"))
        .collect();
    names.sort();
    for path in names {
        let bytes: Arc<[u8]> = fs::read(&path).unwrap().into();
        let report = mw_crystal_io::poscar::read(&bytes);
        let Some((block, structure)) = report.structures().next() else {
            continue;
        };
        let sha = Sha256::of(&bytes);
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let import = Import::new(
            SourceFormat::Poscar,
            file_name.clone(),
            Some(path.display().to_string()),
            sha.clone(),
            block.name.clone(),
            now.clone(),
            structure_sha256(structure),
        );
        add(
            &mut manifest,
            file_name,
            Provenance::Import(import),
            structure.clone(),
        );
        sources.push(Source {
            sha256: sha,
            format: SourceFormat::Poscar,
            bytes,
        });
    }
    let oriented_id = add(
        &mut manifest,
        "金红石（左手系）".into(),
        Provenance::Created(mw_project_file::Created::new(now.clone())),
        oriented_rutile(),
    );
    let mut view = ViewState::default();
    view.open = Some(rutile_id);
    let center = SphereCenter::Atom(AtomRef::new(SiteId(2), 1, [0, -1, 2]));
    view.entries.insert(
        rutile_id,
        EntryView::new(
            Some(Camera::new(0.6, -0.35, 6.25, [0.5, 0.0, -1.25])),
            Some(DisplayModel::SpaceFilling),
            Some(PanelTab::Measurements),
            Some(false),
        )
        .with_display_range(DisplayRange::Sphere {
            center,
            radius: 6.5,
        }),
    );
    let range = DisplayRange::Cells {
        counts: [3, 2, 4],
        boundary: false,
    };
    let centers = vec!["Ti".to_string(), "O".to_string()];
    view.entries.insert(
        oriented_id,
        EntryView::default()
            .with_display_range(range)
            .with_polyhedron_centers(centers),
    );
    Built {
        save: Save {
            manifest,
            structures: files.clone(),
            sources,
            from: None,
            view: Some(view),
        },
        files,
    }
}

fn project_dir(temp: &tempfile::TempDir, name: &str) -> PathBuf {
    temp.path().join(format!("{name}.mwproj"))
}

/// The bytes of every file under `dir`, by relative path.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = fs::read(&path).unwrap();
                files.push((path.strip_prefix(dir).unwrap().to_path_buf(), bytes));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn everything_reads_back_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let path = project_dir(&temp, "a");
    let built = build();
    assert!(built.files.len() > 15, "{}", built.files.len());
    let written = save(&path, &built.save).unwrap();
    assert!(!written.is_migrated());

    let (dir, manifest) = read_manifest(&path).unwrap();
    assert_eq!(manifest, built.save.manifest);
    for (id, file) in &built.files {
        let read = dir.read_structure(*id).unwrap();
        assert_eq!(&read, file, "{id}");
        // Measurements stay as they were, the stale ones included.
        let restored = CrystalDocument::restore(read.structure, read.measurements.clone());
        assert_eq!(restored.annotations(), &read.measurements[..]);
    }
    let stale = built
        .files
        .iter()
        .flat_map(|(_, f)| &f.measurements)
        .filter(|a| a.stale)
        .count();
    assert!(stale > 10, "{stale}");
    let mut poscar = 0;
    for entry in &manifest.entries {
        let Some(import) = entry.provenance.import() else {
            continue;
        };
        let bytes = dir.read_source(import).unwrap();
        let block = import.read_block(&bytes).expect("the block is there");
        let structure = block.structure.expect("it has a structure");
        assert_eq!(structure_sha256(&structure), import.structure_sha256);
        if import.format == SourceFormat::Poscar {
            assert!(import.copy_path().ends_with(".vasp"));
            assert!(path.join(import.copy_path()).exists());
            poscar += 1;
        }
    }
    // Every POSCAR fixture but the VASP 4 one, which needs its elements.
    assert_eq!(poscar, 5);
    assert_eq!(dir.read_view(), built.save.view);

    // The same content gives the same bytes.
    let again = project_dir(&temp, "b");
    save(&again, &built.save).unwrap();
    assert_eq!(snapshot(&path), snapshot(&again));
}

#[test]
fn files_are_written_one_item_per_line() {
    let temp = tempfile::tempdir().unwrap();
    let path = project_dir(&temp, "a");
    let built = build();
    save(&path, &built.save).unwrap();
    let manifest = fs::read_to_string(path.join("project.json")).unwrap();
    assert!(manifest.starts_with(&format!(
        "{{\n  \"format\": \"materials-workbench/project\",\n  \"version\": {VERSION},\n  \"writer\": \"Materials Workbench "
    )));
    let entries = manifest.lines().filter(|l| l.contains("\"kind\"")).count();
    assert_eq!(entries, built.files.len());
    let (supercell, _) = &built.files[1];
    let text = fs::read_to_string(path.join(format!("structures/{supercell}.json"))).unwrap();
    let sites = text.lines().filter(|l| l.contains("\"occupants\"")).count();
    assert_eq!(sites, 47, "one line per remaining site");
    assert!(
        text.contains("\n      \"a\": {\"value\": 9.188},\n"),
        "{text}"
    );
    // An explicit orientation follows the cell parameters on one line; the
    // standard one is not written.
    assert!(!text.contains("orientation"));
    let (oriented, _) = built.files.last().unwrap();
    let text = fs::read_to_string(path.join(format!("structures/{oriented}.json"))).unwrap();
    let line = text
        .lines()
        .find(|l| l.contains("\"orientation\""))
        .expect("an orientation");
    assert!(line.starts_with("      \"orientation\": [["), "{line}");
    assert!(line.ends_with("-1.0]]"), "{line}");
}

#[test]
fn saving_again_writes_only_what_changed() {
    let temp = tempfile::tempdir().unwrap();
    let path = project_dir(&temp, "a");
    let mut built = build();
    save(&path, &built.save).unwrap();
    let (dir, mut manifest) = read_manifest(&path).unwrap();

    // Rename one structure and change another; leave the rest on disk.
    manifest.entries[0].name = "Rutile".into();
    let (changed_id, mut changed) = built.files[2].clone();
    changed.structure.sites.pop();
    let untouched = built.files[3].0;
    let before = fs::read(path.join(format!("structures/{untouched}.json"))).unwrap();
    let second = Save {
        manifest: manifest.clone(),
        structures: vec![(changed_id, changed.clone())],
        sources: Vec::new(),
        from: Some(dir),
        view: None,
    };
    let dir = save(&path, &second).unwrap();
    let (dir, read) = read_manifest(dir.path()).unwrap();
    assert_eq!(read, manifest);
    assert_eq!(dir.read_structure(changed_id).unwrap(), changed);
    let after = fs::read(path.join(format!("structures/{untouched}.json"))).unwrap();
    assert_eq!(before, after);
    // The view state was not given, so it stays.
    assert_eq!(dir.read_view(), built.save.view.take());
}

#[test]
fn files_nothing_refers_to_are_removed() {
    let temp = tempfile::tempdir().unwrap();
    let path = project_dir(&temp, "a");
    let built = build();
    let dir = save(&path, &built.save).unwrap();
    fs::write(path.join("notes.txt"), "mine").unwrap();
    fs::write(path.join("structures/notes.json"), "mine").unwrap();

    // Keep only the samples: every fixture entry and copy goes.
    let mut manifest = built.save.manifest.clone();
    manifest.entries.truncate(3);
    let save_after = Save {
        manifest: manifest.clone(),
        structures: Vec::new(),
        sources: Vec::new(),
        from: Some(dir),
        view: None,
    };
    save(&path, &save_after).unwrap();
    let structures = fs::read_dir(path.join("structures")).unwrap().count();
    assert_eq!(structures, 3 + 1);
    assert_eq!(fs::read_dir(path.join("sources")).unwrap().count(), 0);
    assert!(path.join("notes.txt").exists());
    assert!(path.join("structures/notes.json").exists());
}

#[test]
fn saving_elsewhere_copies_the_project() {
    let temp = tempfile::tempdir().unwrap();
    let first = project_dir(&temp, "a");
    let built = build();
    let dir = save(&first, &built.save).unwrap();
    let (_, manifest) = read_manifest(&first).unwrap();
    let second = project_dir(&temp, "copy");
    let copy = Save {
        manifest,
        structures: Vec::new(),
        sources: Vec::new(),
        from: Some(dir),
        view: built.save.view.clone(),
    };
    save(&second, &copy).unwrap();
    assert_eq!(snapshot(&first), snapshot(&second));
}

#[test]
fn a_new_project_needs_every_entry() {
    let temp = tempfile::tempdir().unwrap();
    let mut built = build();
    let missing = built.save.structures.pop().unwrap().0;
    let error = save(&project_dir(&temp, "a"), &built.save).unwrap_err();
    assert!(
        matches!(error, SaveError::Missing(id) if id == missing),
        "{error}"
    );
}

#[test]
fn unreadable_projects_and_parts_are_reported() {
    let temp = tempfile::tempdir().unwrap();
    let path = project_dir(&temp, "a");
    assert!(matches!(
        read_manifest(&path),
        Err(ReadError::NotAProject(_))
    ));
    let built = build();
    save(&path, &built.save).unwrap();

    // A broken structure file spoils that structure only.
    let (broken, _) = built.files[0];
    let (fine, _) = built.files[1];
    let broken_path = path.join(format!("structures/{broken}.json"));
    let text = fs::read_to_string(&broken_path).unwrap();
    fs::write(&broken_path, text.replacen("\"x,y,z\",", "", 1)).unwrap();
    let (dir, _) = read_manifest(&path).unwrap();
    let error = dir.read_structure(broken).unwrap_err();
    assert!(matches!(error, ReadError::Corrupt { .. }), "{error}");
    assert!(error.to_string().contains("identity"), "{error}");
    assert!(dir.read_structure(fine).is_ok());
    fs::write(&broken_path, text.replacen("4.594", "-4.594", 1)).unwrap();
    assert!(matches!(
        dir.read_structure(broken),
        Err(ReadError::Invalid { .. })
    ));
    // An orientation that is not orthogonal.
    let (oriented, _) = built.files.last().unwrap();
    let oriented_path = path.join(format!("structures/{oriented}.json"));
    let text = fs::read_to_string(&oriented_path).unwrap();
    fs::write(&oriented_path, text.replacen("-1.0]]", "-2.0]]", 1)).unwrap();
    let error = dir.read_structure(*oriented).unwrap_err();
    assert!(matches!(error, ReadError::Corrupt { .. }), "{error}");
    assert!(error.to_string().contains("orthogonal"), "{error}");

    // A changed or missing copy of an imported file.
    let (_, manifest) = read_manifest(&path).unwrap();
    let import = manifest
        .entries
        .iter()
        .find_map(|e| e.provenance.import())
        .unwrap();
    let copy = path.join(import.copy_path());
    fs::write(&copy, "data_changed\n").unwrap();
    assert!(matches!(
        dir.read_source(import),
        Err(ReadError::SourceChanged(_))
    ));
    fs::remove_file(&copy).unwrap();
    assert!(matches!(
        dir.read_source(import),
        Err(ReadError::SourceMissing(_))
    ));

    // A newer format.
    let manifest_path = path.join("project.json");
    let text = fs::read_to_string(&manifest_path).unwrap();
    fs::write(
        &manifest_path,
        text.replace(&format!("\"version\": {VERSION},"), "\"version\": 7,"),
    )
    .unwrap();
    assert!(matches!(
        read_manifest(&path),
        Err(ReadError::Newer { version: 7 })
    ));
    fs::write(
        &manifest_path,
        text.replace("materials-workbench/project", "x"),
    )
    .unwrap();
    assert!(matches!(
        read_manifest(&path),
        Err(ReadError::NotAProject(_))
    ));
}

#[test]
fn unreadable_view_state_is_dropped() {
    let temp = tempfile::tempdir().unwrap();
    let path = project_dir(&temp, "a");
    let built = build();
    let dir = save(&path, &built.save).unwrap();
    assert!(dir.read_view().is_some());
    let view = path.join("view.json");
    let text = fs::read_to_string(&view).unwrap();
    fs::write(&view, text.replace("\"version\": 5", "\"version\": 6")).unwrap();
    assert_eq!(dir.read_view(), None);
    fs::write(&view, "{").unwrap();
    assert_eq!(dir.read_view(), None);
    write_view(&path, &ViewState::default()).unwrap();
    assert_eq!(dir.read_view(), Some(ViewState::default()));
    fs::remove_file(&view).unwrap();
    assert_eq!(dir.read_view(), None);
}

#[test]
fn version_1_view_state_reads_with_the_default_range() {
    let temp = tempfile::tempdir().unwrap();
    let path = project_dir(&temp, "a");
    let dir = save(&path, &build().save).unwrap();
    // As v0.2 wrote it: no display range.
    let entry = EntryId::new();
    let v1 = format!(
        r#"{{
  "version": 1,
  "open": "{entry}",
  "entries": {{
    "{entry}": {{
      "camera": {{ "yaw": 0.6, "pitch": 0.35, "half_height": 6.2, "target": [0.0, 0.0, 0.0] }},
      "display_model": "ball_and_stick",
      "panel": "sites",
      "panel_open": true
    }}
  }}
}}"#
    );
    fs::write(path.join("view.json"), v1).unwrap();
    let view = dir.read_view().expect("a version 1 view");
    assert_eq!(view.open, Some(entry));
    let saved = view.entries[&entry].clone();
    assert_eq!(saved.display_model, Some(DisplayModel::BallAndStick));
    assert_eq!(saved.panel, Some(PanelTab::Sites));
    assert_eq!(saved.display_range, None);
    assert_eq!(saved.polyhedron_centers, None);
    assert_eq!(saved.ellipsoid_probability, None);
    assert!(saved.disorder_groups.is_empty());
    // Written back as version 5, ranges, polyhedra, ellipsoids and disorder
    // groups included.
    let range = DisplayRange::Sphere {
        center: SphereCenter::Point([0.5, 0.25, 0.0]),
        radius: 4.0,
    };
    let mut view = view;
    let mut saved = saved
        .with_display_range(range)
        .with_polyhedron_centers(vec!["Zn".into(), "Si".into(), "Zn".into()])
        .with_ellipsoid_probability(0.3)
        .with_disorder_groups(vec![
            DisorderGroup::new(Some("A".into()), "2".into()),
            DisorderGroup::new(None, "1".into()),
        ]);
    saved.display_model = Some(DisplayModel::Ellipsoids);
    view.entries.insert(entry, saved);
    write_view(&path, &view).unwrap();
    let text = fs::read_to_string(path.join("view.json")).unwrap();
    assert!(text.contains("\"version\": 5"), "{text}");
    assert!(
        text.contains(r#""display_model": "ellipsoids""#)
            && text.contains(r#""ellipsoid_probability": 0.3"#),
        "{text}"
    );
    // Sorted, each once.
    let compact: String = text.split_whitespace().collect();
    assert!(
        compact.contains(r#""polyhedron_centers":["Si","Zn"]"#),
        "{text}"
    );
    assert!(
        text.contains(r#""display_range": {"#) && text.contains(r#""point": ["#),
        "{text}"
    );
    // The unnamed assembly has no `assembly`.
    assert!(
        compact.contains(r#""disorder_groups":[{"assembly":"A","group":"2"},{"group":"1"}]"#),
        "{text}"
    );
    assert_eq!(dir.read_view(), Some(view));
}
