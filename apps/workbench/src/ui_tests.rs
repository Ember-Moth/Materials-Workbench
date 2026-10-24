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
use mw_project::StructureEntryId;
use mw_project_file::{Camera, DisplayModel, EntryView, PanelTab, Provenance, SourceFormat};
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
    let one_cell = document
        .derived()
        .display(document.structure(), &Default::default());
    let atoms: Vec<_> = one_cell.atoms.iter().map(|a| a.atom).collect();
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
    cx.simulate_keystrokes("secondary-s");
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
    cx.simulate_keystrokes("secondary-s");
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

#[gpui_kit::test]
fn poscar_files_are_imported_and_saved_with_their_copy(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let projects = install_samples(temp.path());
    let (workbench, mut cx) = start_with(cx, temp.path(), &projects[1..]);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/mw-crystal-io/tests/fixtures/poscar");
    // A CONTCAR as a VASP run leaves it, named after its folder.
    let run = temp.path().join("TiO2-relax");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::copy(fixtures.join("CONTCAR"), run.join("CONTCAR")).unwrap();
    let notes = temp.path().join("notes.txt");
    std::fs::write(&notes, "not a structure").unwrap();
    let notifications =
        |cx: &mut VisualTestContext| cx.update(|window, cx| window.notifications(cx).len());
    let before = notifications(&mut cx);
    cx.update(|window, cx| {
        workbench.update(cx, |workbench, cx| {
            workbench.import_paths(
                vec![
                    run.join("CONTCAR"),
                    fixtures.join("gaas_primitive_cartesian.vasp"),
                    notes,
                ],
                window,
                cx,
            )
        })
    });
    draw(&mut cx);
    assert_eq!(
        shown(&workbench, &mut cx),
        Some(("钙钛矿氧化物".into(), "TiO2-relax · CONTCAR".into()))
    );
    // The CONTCAR's warning and the text file each get a notification.
    assert_eq!(notifications(&mut cx), before + 2);

    focus(&workspace(&workbench, &mut cx), &mut cx);
    cx.simulate_keystrokes("secondary-s");
    draw(&mut cx);
    assert!(!unsaved(&workbench, &mut cx));

    let (dir, manifest) = mw_project_file::read_manifest(&projects[1]).unwrap();
    let names: Vec<&str> = manifest.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["SrTiO₃", "TiO2-relax · CONTCAR", "gaas_primitive_cartesian"]
    );
    for entry in &manifest.entries[1..] {
        let Provenance::Import(import) = &entry.provenance else {
            panic!("imported");
        };
        assert_eq!(import.format, SourceFormat::Poscar);
        assert!(import.copy_path().ends_with(".vasp"));
        let bytes = dir.read_source(import).unwrap();
        let block = import.read_block(&bytes).unwrap();
        let file = dir.read_structure(entry.id).unwrap();
        assert_eq!(block.structure.as_ref(), Some(&file.structure));
        let codes: Vec<&str> = block.diagnostics.iter().map(|d| d.code()).collect();
        if entry.name.ends_with("CONTCAR") {
            assert_eq!(
                codes,
                [
                    "poscar.selective_dynamics_ignored",
                    "poscar.trailing_content"
                ]
            );
            assert!(file.structure.lattice.orientation().rotation().is_none());
        } else {
            assert!(codes.is_empty(), "{codes:?}");
            // The fcc primitive vectors keep their orientation on disk.
            assert!(file.structure.lattice.orientation().rotation().is_some());
        }
    }
}

#[gpui_kit::test]
fn supercells_join_the_project_as_derived_structures(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let projects = install_samples(temp.path());
    let (workbench, mut cx) = start_with(cx, temp.path(), &projects[1..]);
    let srtio3 = workspace(&workbench, &mut cx);
    cx.update(|window, cx| {
        srtio3.update(cx, |workspace, cx| {
            workspace.make_supercell_of([2, 2, 1], window, cx)
        })
    });
    draw(&mut cx);
    let name = shown(&workbench, &mut cx).expect("the supercell is open").1;
    assert!(name.starts_with("SrTiO₃ 2×2×1 "), "{name}");
    assert!(unsaved(&workbench, &mut cx));
    let names: Vec<String> = workbench.read_with(&cx, |workbench, cx| {
        let project = workbench.catalog(cx).projects()[0].clone();
        project
            .structures()
            .iter()
            .map(|s| s.name().to_string())
            .collect()
    });
    assert_eq!(names, ["SrTiO₃".to_string(), name.clone()]);

    focus(&workspace(&workbench, &mut cx), &mut cx);
    cx.simulate_keystrokes("secondary-s");
    draw(&mut cx);
    assert!(!unsaved(&workbench, &mut cx));

    let (dir, manifest) = mw_project_file::read_manifest(&projects[1]).unwrap();
    let entry = &manifest.entries[1];
    assert_eq!(entry.name, name);
    let Provenance::Derived(derived) = &entry.provenance else {
        panic!("derived");
    };
    assert_eq!(derived.from, manifest.entries[0].id);
    assert_eq!(derived.from_name, "SrTiO₃");
    assert_eq!(
        derived.operation,
        mw_project_file::Operation::MakeSupercell {
            matrix: [[2, 0, 0], [0, 2, 0], [0, 0, 1]]
        }
    );
    let file = dir.read_structure(entry.id).unwrap();
    assert_eq!(file.structure.sites.len(), 20);
    assert!(file.structure.symmetry.is_p1());
}

/// The answer to a tool call, once there is one.
type Answer =
    std::rc::Rc<std::cell::RefCell<Option<Result<rmcp::model::CallToolResult, rmcp::ErrorData>>>>;

/// Starts a tool call through `server` as the MCP client `client` would;
/// the call runs on the workbench, as with a client connected over the
/// channel. A call that waits for the user is answered later.
fn start_call(
    server: &mw_mcp::Server,
    client: &mw_automation::Client,
    tool: &str,
    arguments: serde_json::Value,
    cx: &mut VisualTestContext,
) -> Answer {
    let serde_json::Value::Object(arguments) = arguments else {
        panic!("arguments are an object");
    };
    let answer = Answer::default();
    let (server, client, tool) = (server.clone(), client.clone(), tool.to_owned());
    let slot = answer.clone();
    cx.update(|_, cx| {
        cx.spawn(async move |_| {
            *slot.borrow_mut() = Some(server.call_as(client, &tool, arguments).await);
        })
        .detach()
    });
    draw(cx);
    answer
}

fn answered(answer: &Answer) -> rmcp::model::CallToolResult {
    let answer = answer.borrow_mut().take();
    answer
        .expect("the call was answered")
        .expect("a tool result")
}

/// Calls a tool through `server` as an MCP client would.
fn call_tool(
    server: &mw_mcp::Server,
    tool: &str,
    arguments: serde_json::Value,
    cx: &mut VisualTestContext,
) -> rmcp::model::CallToolResult {
    let client = mw_automation::Client::new("in-process", None);
    answered(&start_call(server, &client, tool, arguments, cx))
}

/// A client's call that succeeds.
fn call_as(
    server: &mw_mcp::Server,
    client: &mw_automation::Client,
    tool: &str,
    arguments: serde_json::Value,
    cx: &mut VisualTestContext,
) -> serde_json::Value {
    let result = answered(&start_call(server, client, tool, arguments, cx));
    assert_ne!(result.is_error, Some(true), "{tool}: {result:#?}");
    result.structured_content.expect("structured")
}

/// The error code of a result.
fn error_code(result: &rmcp::model::CallToolResult) -> String {
    assert_eq!(result.is_error, Some(true), "{result:#?}");
    let text = serde_json::to_value(&result.content[0]).unwrap();
    let error: serde_json::Value = serde_json::from_str(text["text"].as_str().unwrap()).unwrap();
    error["code"].as_str().unwrap().to_owned()
}

/// The output of a call that succeeds.
fn call(
    server: &mw_mcp::Server,
    tool: &str,
    arguments: serde_json::Value,
    cx: &mut VisualTestContext,
) -> serde_json::Value {
    let result = call_tool(server, tool, arguments, cx);
    assert_ne!(result.is_error, Some(true), "{tool}: {result:#?}");
    result.structured_content.expect("structured")
}

/// The code of a call that fails.
fn refusal(
    server: &mw_mcp::Server,
    tool: &str,
    arguments: serde_json::Value,
    cx: &mut VisualTestContext,
) -> String {
    let result = call_tool(server, tool, arguments, cx);
    assert_eq!(result.is_error, Some(true), "{tool}: {result:#?}");
    let text = serde_json::to_value(&result.content[0]).unwrap();
    let error: serde_json::Value = serde_json::from_str(text["text"].as_str().unwrap()).unwrap();
    error["code"].as_str().unwrap().to_owned()
}

/// Milestone M12: an MCP client connected to the application lists its
/// open projects, opens structures in its window and reads them as shown,
/// edits included.
#[gpui_kit::test]
fn mcp_clients_work_on_the_open_projects(cx: &mut TestAppContext) {
    use serde_json::json;

    let temp = tempfile::tempdir().unwrap();
    let projects = install_samples(temp.path());
    let (workbench, mut cx) = start_with(cx, temp.path(), &projects[..1]);
    let (dispatcher, calls) = mw_mcp::dispatcher();
    let _calls = cx.update(|window, cx| {
        workbench.update(cx, |_, cx| crate::mcp::run_calls(calls, window, cx))
    });
    let allowed = temp.path().join("projects");
    let access = mw_mcp::Access::new(std::slice::from_ref(&allowed)).unwrap();
    let server = mw_mcp::Server::dispatching(access, dispatcher);
    let tools: Vec<String> = server.tools().iter().map(|t| t.name.to_string()).collect();
    assert!(tools.contains(&"structure_summary".to_string()));
    assert!(!tools.contains(&"file_open".to_string()));
    assert!(tools.contains(&"structure_set_cell".to_string()));

    let list = call(&server, "project_list", json!({}), &mut cx);
    assert_eq!(list["open"].as_array().unwrap().len(), 1);
    assert_eq!(list["open"][0]["name"], "TiO₂ 相研究");
    assert_eq!(list["open"][0]["structures"], 2);
    assert_eq!(list["recent"][0]["open"], true);

    let structures = call(&server, "structure_list", json!({}), &mut cx);
    let structures = structures["structures"].as_array().unwrap().clone();
    let id = |name: &str| {
        structures
            .iter()
            .find(|s| s["name"] == name)
            .map(|s| s["structure"].clone())
            .unwrap()
    };
    let (rutile, supercell) = (id("金红石"), id("金红石 2×2×2 超胞"));
    assert_eq!(structures[0]["open"], true);
    assert_eq!(structures[1]["open"], false);

    // A structure the user has not opened: the client opens it, and the
    // window shows it.
    let code = refusal(
        &server,
        "structure_summary",
        json!({ "structure": supercell }),
        &mut cx,
    );
    assert_eq!(code, "structure_not_open");
    call(
        &server,
        "structure_open",
        json!({ "structure": supercell }),
        &mut cx,
    );
    assert_eq!(
        shown(&workbench, &mut cx),
        Some(("TiO₂ 相研究".into(), "金红石 2×2×2 超胞".into()))
    );
    let summary = call(
        &server,
        "structure_summary",
        json!({ "structure": supercell }),
        &mut cx,
    );
    assert_eq!(summary["atoms_in_cell"], 48);

    // What the user edits, the client reads.
    let rutile_id = StructureEntryId(rutile.as_u64().unwrap() as u32);
    cx.update(|window, cx| {
        workbench.update(cx, |workbench, cx| {
            workbench.open_structure(rutile_id, window, cx)
        })
    });
    draw(&mut cx);
    let a = CellValues::default().with(CellParam::A, 4.7);
    let shown_rutile = workspace(&workbench, &mut cx);
    execute(
        &shown_rutile,
        Command::Edit(Edit::SetCell { values: a }),
        &mut cx,
    );
    let summary = call(
        &server,
        "structure_summary",
        json!({ "structure": rutile }),
        &mut cx,
    );
    assert_eq!(summary["cell"]["a"], 4.7);
    assert_eq!(summary["cell"]["b"], 4.7);
    let cif = call(
        &server,
        "structure_export",
        json!({ "structure": rutile, "format": "cif" }),
        &mut cx,
    );
    assert!(cif["text"].as_str().unwrap().contains("_cell_length_a"));
    // Still unsaved: clients do not save.
    assert!(unsaved(&workbench, &mut cx));

    // A project from disk joins the open ones without taking the window.
    let opened = call(
        &server,
        "project_open",
        json!({ "path": projects[1] }),
        &mut cx,
    );
    assert_eq!(opened["name"], "钙钛矿氧化物");
    assert_eq!(project_count(&workbench, &mut cx), 2);
    assert_eq!(
        shown(&workbench, &mut cx),
        Some(("TiO₂ 相研究".into(), "金红石".into()))
    );
    let again = call(
        &server,
        "project_open",
        json!({ "path": projects[1] }),
        &mut cx,
    );
    assert_eq!(again["project"], opened["project"]);
    let recent = workbench.read_with(&cx, |workbench, _| workbench.session.recent.clone());
    assert_eq!(recent[0].name, "钙钛矿氧化物");
    // Only inside the directories the client allows.
    let code = refusal(
        &server,
        "project_open",
        json!({ "path": temp.path().join("config") }),
        &mut cx,
    );
    assert_eq!(code, "not_allowed");
}

/// The setting lets MCP clients connect at once, and stops them when it is
/// turned off; it is saved.
#[gpui_kit::test]
fn the_mcp_setting_starts_and_stops_listening(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (workbench, mut cx) = start(cx, temp.path());
    let paths = AppPaths::in_dir(temp.path().join("config"));
    let endpoint = paths.mcp_endpoint();
    let listening = |workbench: &Entity<Workbench>, cx: &mut VisualTestContext| {
        workbench.read_with(cx, |workbench, _| workbench.mcp.is_some())
    };
    assert!(!listening(&workbench, &mut cx));
    assert!(!endpoint.token_file().exists());

    cx.update(|_, cx| crate::settings::set_mcp(true, cx));
    draw(&mut cx);
    assert!(listening(&workbench, &mut cx));
    assert!(endpoint.token_file().exists());
    #[cfg(unix)]
    assert!(endpoint.socket().exists());
    assert!(crate::config::Settings::load(&paths).mcp);

    cx.update(|_, cx| crate::settings::set_mcp(false, cx));
    draw(&mut cx);
    assert!(!listening(&workbench, &mut cx));
    assert!(!endpoint.token_file().exists());
    assert!(!crate::config::Settings::load(&paths).mcp);
}

/// With the setting on from the start and listening impossible (here a
/// file is in the way of the endpoint's directory), the window opens and
/// nothing listens.
#[gpui_kit::test]
fn a_failure_to_listen_at_start_is_no_crash(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let endpoint = AppPaths::in_dir(temp.path().join("config")).mcp_endpoint();
    std::fs::create_dir_all(temp.path().join("config")).unwrap();
    std::fs::write(endpoint.dir(), "").unwrap();
    cx.update(|cx| cx.set_global(crate::settings::McpSetting(true)));
    let (workbench, mut cx) = start(cx, temp.path());
    assert!(workbench.read_with(&cx, |workbench, _| workbench.mcp.is_none()));
    assert!(!endpoint.token_file().exists());
    assert_eq!(project_count(&workbench, &mut cx), 2);
}

/// The application serving MCP clients: a server whose calls run on the
/// workbench, allowed into the projects' directory.
fn serve(
    workbench: &Entity<Workbench>,
    allowed: &Path,
    cx: &mut VisualTestContext,
) -> (mw_mcp::Server, gpui_kit::Task<()>) {
    let (dispatcher, calls) = mw_mcp::dispatcher();
    let task = cx.update(|window, cx| {
        workbench.update(cx, |_, cx| crate::mcp::run_calls(calls, window, cx))
    });
    let access = mw_mcp::Access::new(&[allowed.to_path_buf()]).unwrap();
    (mw_mcp::Server::dispatching(access, dispatcher), task)
}

/// Milestone M18: a client's edits go through the workspace onto the undo
/// stack as the client's, within what the settings allow; new structures
/// join the project unsaved; a file is written only once the user confirms
/// in the window; the audit log is saved with the project, which the
/// application holds locked.
#[gpui_kit::test]
fn mcp_clients_edit_in_the_window(cx: &mut TestAppContext) {
    use mw_automation::Client;
    use serde_json::json;

    let temp = tempfile::tempdir().unwrap();
    let projects = install_samples(temp.path());
    let (workbench, mut cx) = start_with(cx, temp.path(), &projects[..1]);
    // Another process cannot write the project the application has open.
    assert!(mw_project_file::ProjectLock::acquire(&projects[0], "test").is_err());
    let allowed = temp.path().join("projects");
    let (server, _calls) = serve(&workbench, &allowed, &mut cx);
    let claude = Client::new("claude-code", Some("Claude Code".into()));
    let call = |tool: &str, arguments, cx: &mut VisualTestContext| {
        call_as(&server, &claude, tool, arguments, cx)
    };

    let rutile = workbench.read_with(&cx, |workbench, _| workbench.current.unwrap());
    let rutile = json!(rutile.0);
    call("structure_open", json!({ "structure": rutile }), &mut cx);
    let edit = json!({ "structure": rutile, "values": [{ "parameter": "a", "value": 4.7 }] });
    call("structure_set_cell", edit.clone(), &mut cx);
    let shown = workspace(&workbench, &mut cx);
    shown.read_with(&cx, |workspace, _| {
        let crystal = workspace.crystal();
        assert_eq!(crystal.structure().lattice.params().a.value(), 4.7);
        assert_eq!(crystal.undo_agent().map(|a| a.name()), Some("Claude Code"));
    });
    assert!(unsaved(&workbench, &mut cx));
    // The user can undo it like their own edit, and so can the client.
    call("history_undo", json!({ "structure": rutile }), &mut cx);
    shown.read_with(&cx, |workspace, _| {
        assert_eq!(
            workspace.crystal().structure().lattice.params().a.value(),
            4.594
        );
    });
    // The client is remembered, and the user can take away its permission.
    let ai = cx.update(|_, cx| crate::settings::AiSetting::get(cx));
    assert_eq!(ai.clients[0].name, "claude-code");
    let mut forbidden = ai.clone();
    forbidden.clients[0].modify_data = Some(false);
    cx.update(|_, cx| crate::settings::set_ai(forbidden, cx));
    let refused = answered(&start_call(
        &server,
        &claude,
        "structure_set_cell",
        edit.clone(),
        &mut cx,
    ));
    assert_eq!(error_code(&refused), "not_permitted");
    // Other clients follow the settings for all.
    let other = Client::new("other", None);
    let allowed_edit = answered(&start_call(
        &server,
        &other,
        "structure_set_cell",
        edit,
        &mut cx,
    ));
    assert_ne!(allowed_edit.is_error, Some(true));
    cx.update(|_, cx| crate::settings::set_ai(ai, cx));

    // A supercell joins the project after its source, shown and unsaved.
    let doubled = json!({
        "structure": rutile,
        "matrix": [[1, 0, 0], [0, 1, 0], [0, 0, 2]],
        "name": "",
    });
    let made = call("structure_make_supercell", doubled, &mut cx);
    // In the interface language, English in tests.
    assert_eq!(made["name"], "金红石 1×1×2 supercell");
    let made_id = StructureEntryId(made["structure"].as_u64().unwrap() as u32);
    assert_eq!(workbench.read_with(&cx, |w, _| w.current), Some(made_id));
    // An import goes into the project the client names.
    let cif = allowed.join("rutile.cif");
    std::fs::write(&cif, RUTILE_CIF).unwrap();
    let list = call("project_list", json!({}), &mut cx);
    let project = list["open"][0]["project"].clone();
    let imported = call(
        "file_import",
        json!({ "path": cif, "project": project }),
        &mut cx,
    );
    assert_eq!(imported["structures"][0]["name"], "rutile");

    // Writing a file waits for the user's answer in the window.
    let target = allowed.join("POSCAR");
    let export =
        json!({ "structure": rutile, "format": "poscar", "path": target, "overwrite": false });
    let pending = start_call(
        &server,
        &claude,
        "structure_export_file",
        export.clone(),
        &mut cx,
    );
    assert!(pending.borrow().is_none());
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    cx.simulate_keystrokes("escape");
    draw(&mut cx);
    assert_eq!(error_code(&answered(&pending)), "not_confirmed");
    assert!(!target.exists());
    let pending = start_call(&server, &claude, "structure_export_file", export, &mut cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    assert_ne!(answered(&pending).is_error, Some(true));
    assert!(
        std::fs::read_to_string(&target)
            .unwrap()
            .starts_with("金红石 · TiO₂ 相研究")
    );

    // Saving writes the structures and the client's calls to the log.
    let saved = workbench.update_in(&mut cx, |workbench, window, cx| {
        let project = workbench.active_project.unwrap();
        workbench.save_project(project, false, window, cx)
    });
    draw(&mut cx);
    drop(saved);
    assert!(!unsaved(&workbench, &mut cx));
    let log = mw_project_file::read_audit(&projects[0]).unwrap();
    let calls: Vec<&str> = log.iter().map(|r| r.capability.as_str()).collect();
    assert!(calls.contains(&"structure.make_supercell"), "{calls:?}");
    assert!(calls.contains(&"structure.export_file"), "{calls:?}");
    let refused = log
        .iter()
        .find(|r| r.result.code.as_deref() == Some("not_permitted"))
        .unwrap();
    assert_eq!(refused.client.title.as_deref(), Some("Claude Code"));
    let written = log
        .iter()
        .find(|r| r.capability == "structure.export_file" && r.result.ok)
        .unwrap();
    assert!(written.confirmed);
    let (_, manifest) = mw_project_file::read_manifest(&projects[0]).unwrap();
    let names: Vec<&str> = manifest.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "金红石",
            "金红石 2×2×2 超胞",
            "金红石 1×1×2 supercell",
            "rutile"
        ]
    );
}

/// The rutile CIF of the sample project.
const RUTILE_CIF: &str = include_str!("../../../crates/mw-crystal-io/tests/fixtures/rutile.cif");

/// Milestone M18, the view tools: a client selects atoms, switches the
/// display model, resets the camera and gets a picture of the structure,
/// which the window shows; no data changes, and no permission is needed.
#[gpui_kit::test]
fn mcp_clients_change_the_view(cx: &mut TestAppContext) {
    use mw_automation::Client;
    use mw_crystal::{AtomRef, SiteId, Target};
    use mw_project_file::{Camera, DisplayModel, EntryView};
    use serde_json::json;

    let temp = tempfile::tempdir().unwrap();
    let projects = install_samples(temp.path());
    let (workbench, mut cx) = start_with(cx, temp.path(), &projects[..1]);
    let (server, _calls) = serve(&workbench, &temp.path().join("projects"), &mut cx);
    // A client the user allowed nothing still changes the view.
    let mut ai = cx.update(|_, cx| crate::settings::AiSetting::get(cx));
    (ai.modify_data, ai.write_files) = (false, false);
    cx.update(|_, cx| crate::settings::set_ai(ai, cx));
    let client = Client::new("viewer", None);
    let call = |tool: &str, arguments, cx: &mut VisualTestContext| {
        call_as(&server, &client, tool, arguments, cx)
    };

    let rutile = workbench.read_with(&cx, |workbench, _| workbench.current.unwrap());
    let structures = call("structure_list", json!({}), &mut cx);
    let supercell = structures["structures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "金红石 2×2×2 超胞")
        .map(|s| s["structure"].clone())
        .unwrap();
    call("structure_open", json!({ "structure": supercell }), &mut cx);
    let shown_supercell = workspace(&workbench, &mut cx);

    // The view tools show the structure they are about.
    let selected = call(
        "view_select",
        json!({
            "structure": rutile.0,
            "sites": [2],
            "atoms": [{ "site": 1, "member": 1, "cell": [0, 0, 0] }],
        }),
        &mut cx,
    );
    assert_eq!(selected, json!({ "sites": [2, 1], "atoms": 1 }));
    assert_eq!(workbench.read_with(&cx, |w, _| w.current), Some(rutile));
    let shown = workspace(&workbench, &mut cx);
    assert_ne!(shown, shown_supercell);
    let ti = AtomRef::new(SiteId(1), 1, [0, 0, 0]);
    assert_eq!(
        shown.read_with(&cx, |w, _| w.selected()),
        [Target::Site(SiteId(2)), Target::Atom(ti)]
    );

    call(
        "view_set_display_model",
        json!({ "structure": rutile.0, "model": "space_filling" }),
        &mut cx,
    );
    let view = |cx: &mut VisualTestContext| shown.read_with(cx, |w, cx| w.view(cx));
    assert_eq!(
        view(&mut cx).display_model,
        Some(DisplayModel::SpaceFilling)
    );

    let framed = view(&mut cx).camera.unwrap();
    let turned = EntryView::new(
        Some(Camera::new(
            1.0,
            0.4,
            framed.half_height * 2.0,
            framed.target,
        )),
        None,
        None,
        None,
    );
    shown.update(&mut cx, |w, cx| w.set_view(&turned, cx));
    assert_ne!(view(&mut cx).camera, Some(framed));
    call("view_reset", json!({ "structure": rutile.0 }), &mut cx);
    // The default angle, and the zoom that takes in the space-filling atoms.
    let reset = view(&mut cx).camera.unwrap();
    assert_eq!((reset.yaw, reset.pitch), (framed.yaw, framed.pitch));
    assert!(reset.half_height < framed.half_height * 2.0);

    // A picture of what the window shows, at the size asked for.
    let picture = answered(&start_call(
        &server,
        &client,
        "view_snapshot",
        json!({ "structure": rutile.0, "width": 320, "height": 240 }),
        &mut cx,
    ));
    assert_ne!(picture.is_error, Some(true), "{picture:#?}");
    assert_eq!(
        picture.structured_content,
        Some(json!({ "width": 320, "height": 240 }))
    );
    let image = serde_json::to_value(picture.content.last().unwrap()).unwrap();
    assert_eq!(image["type"], "image");
    assert_eq!(image["mimeType"], "image/png");
    // The PNG signature, in base64.
    assert!(image["data"].as_str().unwrap().starts_with("iVBORw0KGgo"));
    // Or at the viewport's size.
    let own = call(
        "view_snapshot",
        json!({ "structure": rutile.0, "width": 0, "height": 0 }),
        &mut cx,
    );
    let size = shown.read_with(&cx, |w, cx| w.viewport_size(cx)).unwrap();
    assert_eq!(
        (own["width"].as_u64(), own["height"].as_u64()),
        (Some(size.0.into()), Some(size.1.into()))
    );

    // Nothing to save: the view is not data.
    assert!(!unsaved(&workbench, &mut cx));
    let crystal_revision = shown.read_with(&cx, |w, _| w.crystal().revision());
    assert_eq!(crystal_revision, 0);
}
