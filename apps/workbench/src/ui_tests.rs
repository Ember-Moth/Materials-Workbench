//! UI integration tests of the shell (`docs/development.md` §4): the
//! workbench in a headless window with its own configuration directory,
//! opening, saving, closing and reopening real project directories.
//! Driven through keystrokes and the workbench's own actions; GPUI Kit's
//! interaction helpers are unavailable for the reason given in
//! `crates/mw-structure/src/ui_tests.rs`.

use std::path::{Path, PathBuf};

use gpui_kit::component::WindowExt as _;
use gpui_kit::{
    AppContext as _, Bounds, Entity, Focusable as _, TestAppContext, VisualTestContext,
    WindowBounds, WindowOptions, px, size,
};
use mw_crystal::{CellParam, CellValues, Command, CrystalDocument, Edit, Measurement};
use mw_project_file::{Camera, DisplayModel, EntryView, PanelTab, Provenance};
use mw_structure::StructureWorkspace;

use crate::actions::CloseProject;
use crate::config::{AppPaths, Session};
use crate::workbench::Workbench;

/// Installs the sample projects under `dir/projects` and returns their
/// paths: `tio2.mwproj`, then `perovskite.mwproj`.
fn install_samples(dir: &Path) -> Vec<PathBuf> {
    let projects = dir.join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    mw_project_file::samples::install(&projects).unwrap()
}

/// Starts the workbench with `dir/config` as its configuration directory,
/// as the application starts, and lets it read what it opens.
fn start(cx: &mut TestAppContext, dir: &Path) -> (Entity<Workbench>, VisualTestContext) {
    let paths = AppPaths::in_dir(dir.join("config"));
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::actions::bind_keys(cx);
        mw_structure::bind_keys(cx);
        cx.set_global(paths);
    });
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            Default::default(),
            size(px(1280.), px(800.)),
        ))),
        ..Default::default()
    };
    let (window, workbench) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Workbench::new(window, cx))
        })
        .expect("open a test window")
    });
    let mut cx = VisualTestContext::from_window(window, cx);
    draw(&mut cx);
    (workbench, cx)
}

/// Starts with a session that had `open` open.
fn start_with(
    cx: &mut TestAppContext,
    dir: &Path,
    open: &[PathBuf],
) -> (Entity<Workbench>, VisualTestContext) {
    let session = Session {
        open: open.to_vec(),
        active: open.first().cloned(),
        ..Session::default()
    };
    session.save(&AppPaths::in_dir(dir.join("config")));
    start(cx, dir)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
}

fn workspace(
    workbench: &Entity<Workbench>,
    cx: &mut VisualTestContext,
) -> Entity<StructureWorkspace> {
    workbench.read_with(cx, |workbench, _| {
        workbench.workspace().cloned().expect("a structure is open")
    })
}

/// The open structure's project and name.
fn shown(workbench: &Entity<Workbench>, cx: &mut VisualTestContext) -> Option<(String, String)> {
    workbench.read_with(cx, |workbench, cx| {
        let (project, entry) = workbench.catalog(cx).structure(workbench.current?)?;
        Some((project.name().to_string(), entry.name().to_string()))
    })
}

fn project_count(workbench: &Entity<Workbench>, cx: &mut VisualTestContext) -> usize {
    workbench.read_with(cx, |workbench, cx| workbench.catalog(cx).projects().len())
}

/// Whether the open structure's project has unsaved changes, as the header
/// says.
fn unsaved(workbench: &Entity<Workbench>, cx: &mut VisualTestContext) -> bool {
    workbench.read_with(cx, |workbench, cx| {
        let project = workbench.active_project.expect("a project is active");
        workbench.is_project_modified(project, cx)
    })
}

fn execute(workspace: &Entity<StructureWorkspace>, command: Command, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace
                .execute(command, window, cx)
                .expect("the command applies")
        })
    });
    draw(cx);
}

fn focus(workspace: &Entity<StructureWorkspace>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let handle = workspace.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    });
    draw(cx);
}

#[gpui_kit::test]
fn the_first_start_opens_the_sample_projects(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (workbench, mut cx) = start(cx, temp.path());
    assert_eq!(project_count(&workbench, &mut cx), 2);
    assert_eq!(
        shown(&workbench, &mut cx),
        Some(("TiO₂ 相研究".into(), "金红石".into()))
    );
    assert!(
        temp.path()
            .join("config/samples/tio2.mwproj/project.json")
            .exists()
    );
    assert!(!unsaved(&workbench, &mut cx));
}

/// Roadmap M6: closing and reopening a project restores it completely.
#[gpui_kit::test]
fn a_project_comes_back_as_it_was_saved(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let projects = install_samples(temp.path());
    let (workbench, mut cx) = start_with(cx, temp.path(), &projects[..1]);
    assert_eq!(
        shown(&workbench, &mut cx),
        Some(("TiO₂ 相研究".into(), "金红石".into()))
    );
    let rutile = workspace(&workbench, &mut cx);

    // An edit, a measurement, and another way of looking at it.
    let a = CellValues::default().with(CellParam::A, 4.7);
    execute(&rutile, Command::Edit(Edit::SetCell { values: a }), &mut cx);
    let (file, _) = rutile.read_with(&cx, |workspace, _| workspace.file_to_save());
    let document = CrystalDocument::new(file.structure);
    let atoms: Vec<_> = document.derived().atoms.iter().map(|a| a.atom).collect();
    let distance = Measurement::Distance([atoms[0], atoms[1]]);
    execute(&rutile, Command::AddMeasurement(distance), &mut cx);
    let view = EntryView::new(
        Some(Camera::new(1.25, -0.5, 4.5, [0.0; 3])),
        Some(DisplayModel::SpaceFilling),
        Some(PanelTab::Sites),
        Some(false),
    );
    cx.update(|_, cx| rutile.update(cx, |workspace, cx| workspace.set_view(&view, cx)));
    draw(&mut cx);
    assert!(unsaved(&workbench, &mut cx));
    let view = cx.update(|_, cx| rutile.read(cx).view(cx));

    focus(&rutile, &mut cx);
    cx.simulate_keystrokes("cmd-s");
    draw(&mut cx);
    assert!(!unsaved(&workbench, &mut cx));
    assert!(!rutile.read_with(&cx, |workspace, _| workspace.is_modified()));

    cx.update(|window, cx| {
        workbench.update(cx, |workbench, cx| {
            workbench.close_active_project(&CloseProject, window, cx)
        })
    });
    draw(&mut cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(project_count(&workbench, &mut cx), 0);
    assert_eq!(shown(&workbench, &mut cx), None);

    cx.update(|window, cx| {
        workbench.update(cx, |workbench, cx| {
            workbench.open_projects(projects[..1].to_vec(), None, window, cx)
        })
    });
    draw(&mut cx);
    assert_eq!(
        shown(&workbench, &mut cx),
        Some(("TiO₂ 相研究".into(), "金红石".into()))
    );
    let reopened = workspace(&workbench, &mut cx);
    assert_ne!(reopened, rutile);
    let (file, _) = reopened.read_with(&cx, |workspace, _| workspace.file_to_save());
    assert_eq!(file.structure.lattice.params().a.value(), 4.7);
    assert_eq!(file.measurements.len(), 1);
    assert_eq!(file.measurements[0].measurement, distance);
    assert_eq!(
        file.edits,
        [
            Command::Edit(Edit::SetCell { values: a }),
            Command::AddMeasurement(distance)
        ]
    );
    assert_eq!(cx.update(|_, cx| reopened.read(cx).view(cx)), view);
    assert!(!unsaved(&workbench, &mut cx));
}

#[gpui_kit::test]
fn closing_with_unsaved_changes_asks_first(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let projects = install_samples(temp.path());
    let (workbench, mut cx) = start_with(cx, temp.path(), &projects);
    let rutile = workspace(&workbench, &mut cx);
    let a = CellValues::default().with(CellParam::A, 4.7);
    execute(&rutile, Command::Edit(Edit::SetCell { values: a }), &mut cx);

    cx.update(|window, cx| {
        workbench.update(cx, |workbench, cx| {
            workbench.close_active_project(&CloseProject, window, cx)
        })
    });
    draw(&mut cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(project_count(&workbench, &mut cx), 2);
    // Escape cancels: nothing is closed, nothing is lost.
    cx.simulate_keystrokes("escape");
    draw(&mut cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(project_count(&workbench, &mut cx), 2);
    assert!(unsaved(&workbench, &mut cx));

    // Quitting asks too, and does not quit yet.
    let quit = cx.update(|window, cx| {
        workbench.update(cx, |workbench, cx| workbench.request_quit(window, cx))
    });
    assert!(!quit);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
}

#[gpui_kit::test]
fn imported_files_are_saved_with_their_copy(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let projects = install_samples(temp.path());
    let (workbench, mut cx) = start_with(cx, temp.path(), &projects[1..]);
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/mw-crystal-io/tests/fixtures/zincite_legacy.cif");
    cx.update(|window, cx| {
        workbench.update(cx, |workbench, cx| {
            workbench.import_paths(vec![fixture], window, cx)
        })
    });
    draw(&mut cx);
    assert_eq!(
        shown(&workbench, &mut cx),
        Some(("钙钛矿氧化物".into(), "zincite_legacy".into()))
    );
    assert!(unsaved(&workbench, &mut cx));

    focus(&workspace(&workbench, &mut cx), &mut cx);
    cx.simulate_keystrokes("cmd-s");
    draw(&mut cx);
    assert!(!unsaved(&workbench, &mut cx));

    let (dir, manifest) = mw_project_file::read_manifest(&projects[1]).unwrap();
    let entry = manifest
        .entries
        .iter()
        .find(|e| e.name == "zincite_legacy")
        .expect("saved in the project");
    let Provenance::Import(import) = &entry.provenance else {
        panic!("imported");
    };
    assert_eq!(import.file_name, "zincite_legacy.cif");
    let bytes = dir.read_source(import).unwrap();
    let block = import.read_block(&bytes).unwrap();
    assert!(
        !block.diagnostics.is_empty(),
        "the legacy file has diagnostics"
    );
    let file = dir.read_structure(entry.id).unwrap();
    assert_eq!(block.structure, Some(file.structure));
}
