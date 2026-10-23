//! Development aid: walks through the main screens and renders each to a PNG
//! with `Window::render_to_image`, which needs no screen-recording
//! permission. Enabled by the `screenshot` feature and `MW_SCREENSHOT_DIR`.
//! The tour draws the window itself, so a window that is not in front still
//! gives current frames.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::WindowExt;
use gpui_kit::*;
use mw_crystal::{
    AtomRef, CellParam, CellValues, Command, Edit, Measured, SiteId, Species, Target,
};
use mw_project::StructureEntryId;
use mw_structure::import::OpenedFile;

use crate::workbench::Workbench;
use crate::{palette, settings};

/// Time for a step's background work (reading a project, a structure or a
/// file) to finish before the frame is waited for.
const SETTLE: Duration = Duration::from_millis(500);
/// How long to wait at most for the viewport's newest frame.
const FRAME_TIMEOUT: Duration = Duration::from_secs(4);
const POLL: Duration = Duration::from_millis(50);

type Step = fn(&mut Workbench, &mut Window, &mut Context<Workbench>);

/// A CIF from the importer's test fixtures.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/mw-crystal-io/tests/fixtures")
        .join(name)
}

/// A pointer action on the atom displayed at a fractional position.
#[derive(Clone, Copy)]
enum Pointer {
    Click([f64; 3]),
    ShiftClick([f64; 3]),
    Hover([f64; 3]),
    /// Press on the atom and drag by this many logical pixels.
    Drag([f64; 3], f32, f32),
}

/// Sends real mouse events to the atoms' positions in the window, so picking
/// goes through the viewport's ID texture as it does for users. Deferred
/// until the step's update of the workbench has ended, because the events
/// reach other views.
fn point_at(workbench: &Workbench, actions: &'static [Pointer], window: &Window, cx: &mut App) {
    let Some(workspace) = workbench.workspace().cloned() else {
        return;
    };
    window.defer(cx, move |window, cx| {
        for action in actions {
            let (Pointer::Click(at)
            | Pointer::ShiftClick(at)
            | Pointer::Hover(at)
            | Pointer::Drag(at, _, _)) = *action;
            let Some(position) = workspace.read(cx).displayed_atom_position(at, cx) else {
                log::warn!("screenshot tour: no atom displayed at {at:?}");
                continue;
            };
            let modifiers = Modifiers {
                shift: matches!(action, Pointer::ShiftClick(_)),
                ..Modifiers::default()
            };
            let mut release_at = position;
            match *action {
                Pointer::Hover(_) => {
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position,
                            pressed_button: None,
                            modifiers,
                        }),
                        cx,
                    );
                    continue;
                }
                Pointer::Drag(_, dx, dy) => release_at = position + point(px(dx), px(dy)),
                Pointer::Click(_) | Pointer::ShiftClick(_) => {}
            }
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            if release_at != position {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: release_at,
                        pressed_button: Some(MouseButton::Left),
                        modifiers,
                    }),
                    cx,
                );
            }
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: release_at,
                    modifiers,
                    click_count: 1,
                }),
                cx,
            );
        }
    });
}

/// Moves focus to the open structure, so that dispatched actions reach it.
/// A structure switched to must have been drawn first: steps whose name
/// starts with `_` set up without a screenshot.
fn focus_workspace(workbench: &Workbench, window: &mut Window, cx: &mut App) {
    if let Some(workspace) = workbench.workspace() {
        workspace.focus_handle(cx).focus(window, cx);
    }
}

/// Runs a command on the open structure, as its fields do.
fn execute(workbench: &Workbench, command: Command, window: &mut Window, cx: &mut App) {
    if let Some(workspace) = workbench.workspace().cloned() {
        workspace.update(cx, |workspace, cx| {
            let _ = workspace.execute(command, window, cx);
        });
    }
}

/// Rutile atoms around the body-center Ti: two apical O and one equatorial O.
const TI_CENTER: [f64; 3] = [0.5, 0.5, 0.5];
const O_APICAL: [f64; 3] = [0.8048, 0.1952, 0.5];
const O_APICAL_OTHER: [f64; 3] = [0.1952, 0.8048, 0.5];
const O_EQUATORIAL: [f64; 3] = [0.6952, 0.6952, 1.0];

const TOUR: &[(&str, Step)] = &[
    ("01-main", |_, _, _| {}),
    ("02-sites", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowSitesPanel), cx)
    }),
    // Turn the view closer to the c axis, where an O no longer hides the
    // body-center Ti; click that Ti, add an apical O, hover over the other.
    ("03-select-atoms", |workbench, window, cx| {
        const ACTIONS: &[Pointer] = &[
            Pointer::Drag(TI_CENTER, 44., -19.),
            Pointer::Click(TI_CENTER),
            Pointer::ShiftClick(O_APICAL),
            Pointer::Hover(O_APICAL_OTHER),
        ];
        point_at(workbench, ACTIONS, window, cx);
    }),
    // A distance and an angle (O–Ti, then O–Ti–O), then a second distance
    // whose two atoms stay picked, waiting for a possible third.
    ("04-measure", |workbench, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ToggleMeasure), cx);
        const ACTIONS: &[Pointer] = &[
            Pointer::Click(O_APICAL),
            Pointer::Click(TI_CENTER),
            Pointer::Click(O_EQUATORIAL),
            Pointer::Click(TI_CENTER),
            Pointer::Click(O_EQUATORIAL),
        ];
        point_at(workbench, ACTIONS, window, cx);
    }),
    ("05-palette", |workbench, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ToggleMeasure), cx);
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
        workbench.toggle_command_palette(&crate::actions::ToggleCommandPalette, window, cx);
    }),
    ("06-delete-dialog", |_, window, cx| {
        window.close_dialog(cx);
        window.dispatch_action(Box::new(mw_structure::DeleteSelection), cx);
    }),
    ("07-dark", |_, window, cx| {
        window.close_dialog(cx);
        settings::set_dark_mode(true, window, cx);
    }),
    ("08-english", |_, window, cx| {
        settings::set_dark_mode(false, window, cx);
        settings::set_language("en", cx);
    }),
    ("09-srtio3", |workbench, window, cx| {
        settings::set_language("zh-CN", cx);
        workbench.open_structure(StructureEntryId(3), window, cx);
    }),
    // Editing SrTiO₃. The cell is cubic: a, b and c change together.
    ("10-edit-cell", |workbench, window, cx| {
        let values = CellValues::default().with(CellParam::A, 4.2);
        execute(
            workbench,
            Command::Edit(Edit::SetCell { values }),
            window,
            cx,
        );
    }),
    // The workspace opened on Sr1; the dialog adds an element to it.
    ("11-add-occupant-dialog", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::AddMixedOccupant), cx)
    }),
    // Ba beside Sr: the occupancies add up to 1.3, and a warning says so.
    ("12-mixed-occupancy", |workbench, window, cx| {
        window.close_dialog(cx);
        let edit = Edit::AddOccupant {
            site: SiteId(1),
            species: Species::element("Ba"),
            occupancy: Measured::exact(0.3),
            label: None,
        };
        execute(workbench, Command::Edit(edit), window, cx);
    }),
    // Ti1 has one atom in the cell, so deleting it deletes the site
    // without asking; a notification names what went.
    ("13-select-ti", |workbench, window, cx| {
        point_at(workbench, &[Pointer::Click([0.5, 0.5, 0.5])], window, cx);
    }),
    ("14-delete-site", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::DeleteSelection), cx)
    }),
    // Undo the deletion, the occupant and the cell: SrTiO₃ as it was.
    ("15-undo", |_, window, cx| {
        for _ in 0..3 {
            window.dispatch_action(Box::new(mw_structure::Undo), cx);
        }
    }),
    // The delete dialog's default for one O of SrTiO₃: convert to P1, then
    // delete that atom alone, as one step.
    ("16-p1-delete", |workbench, window, cx| {
        let Some(workspace) = workbench.workspace().cloned() else {
            return;
        };
        let (file, _) = workspace.read(cx).file_to_save();
        let one = Target::Atom(AtomRef::new(SiteId(3), 2, [0; 3]));
        if let Ok(edit) = mw_crystal::expand_and_remove(&file.structure, &[one]) {
            execute(workbench, Command::Edit(edit), window, cx);
        }
        window.dispatch_action(Box::new(mw_structure::ShowSitesPanel), cx);
    }),
    ("17-supercell", |workbench, window, cx| {
        workbench.open_structure(StructureEntryId(2), window, cx);
    }),
    // ⌘S: the measurements made on rutile are saved, its unsaved mark goes.
    ("18-saved", |workbench, window, cx| {
        workbench.save_active_project(&crate::actions::SaveProject, window, cx);
    }),
    // Opening files goes through the same path as the file dialog and drops.
    // Several at once, as when dropping a selection: the first opens, the
    // rest join the open structure's project, unsaved until ⌘S.
    ("19-import-files", |workbench, window, cx| {
        let files = [
            "bst_su_aniso.cif",
            "rutile.cif",
            "SrTiO3.cif",
            "multi_block.cif",
        ];
        workbench.import_paths(files.map(fixture).to_vec(), window, cx);
    }),
    // Entry 5 is the imported rutile, after the three samples and BST.
    ("20-imported-rutile", |workbench, window, cx| {
        workbench.open_structure(StructureEntryId(5), window, cx);
    }),
    ("21-import-zincite", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("zincite_legacy.cif")], window, cx);
    }),
    ("22-import-diagnostics", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowDiagnosticsPanel), cx)
    }),
    // Sites snapped onto special positions, rows merged and dropped.
    ("23-import-snapped", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("rutile_near_special.cif")], window, cx);
    }),
    ("24-snapped-diagnostics", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowDiagnosticsPanel), cx)
    }),
    ("25-import-failed", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("malformed.cif")], window, cx);
    }),
    ("26-import-errors", |_, window, cx| {
        OpenedFile::read(fixture("malformed.cif")).show_errors(window, cx);
    }),
    // Converting rutile to P1: the dialog, then the six sites it gives.
    ("_show-rutile", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.open_structure(StructureEntryId(1), window, cx);
    }),
    ("27-convert-dialog", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
        window.dispatch_action(Box::new(mw_structure::ConvertToP1), cx);
    }),
    ("28-converted", |workbench, window, cx| {
        window.close_dialog(cx);
        execute(workbench, Command::Edit(Edit::ExpandToP1), window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowSitesPanel), cx);
    }),
    // Back to P4₂/mnm in one undo, then the supercell dialog.
    ("29-supercell-dialog", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::Undo), cx);
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
        window.dispatch_action(Box::new(mw_structure::MakeSupercell), cx);
    }),
    // A new derived structure after rutile's other supercell, opened.
    ("30-supercell-made", |workbench, window, cx| {
        window.close_dialog(cx);
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                workspace.make_supercell_of([3, 3, 1], window, cx)
            });
        }
    }),
    // The imports and the supercell are not saved yet: closing their
    // project asks first.
    ("31-unsaved-dialog", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.close_active_project(&crate::actions::CloseProject, window, cx);
    }),
    ("32-settings", |_, window, cx| {
        window.close_dialog(cx);
        settings::open(window, cx)
    }),
    ("33-no-sidebar", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.set_sidebar_open(false, cx);
    }),
    // The structure from step 23 (entry 10) shares a site between Ti and
    // V, so the formats that list atoms are unavailable and say why.
    ("_show-snapped", |workbench, window, cx| {
        workbench.set_sidebar_open(true, cx);
        workbench.open_structure(StructureEntryId(10), window, cx);
    }),
    ("34-export-dialog", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ExportStructure), cx);
    }),
    // The empty state lists the recent projects.
    ("35-empty", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.close_structure(window, cx);
    }),
    // A new project is untitled and empty until files are opened into it.
    ("36-new-project", |workbench, window, cx| {
        workbench.new_project(&crate::actions::NewProject, window, cx);
    }),
];

/// A fresh directory for the tour's settings, session and sample projects
/// (`<MW_SCREENSHOT_DIR>/config`), so that every run starts as a first
/// start and never touches the user's own files.
pub fn paths() -> Option<crate::config::AppPaths> {
    let dir = std::env::var_os("MW_SCREENSHOT_DIR").map(PathBuf::from)?;
    let config = dir.join("config");
    let _ = std::fs::remove_dir_all(&config);
    Some(crate::config::AppPaths::in_dir(config))
}

pub fn start(window: AnyWindowHandle, workbench: Entity<Workbench>, cx: &mut App) {
    let Some(dir) = std::env::var_os("MW_SCREENSHOT_DIR").map(PathBuf::from) else {
        return;
    };
    // Keep the commands' palette entry referenced so the tour's dialogs match
    // what users see.
    let _ = palette::open;
    cx.spawn(async move |cx| {
        if let Err(e) = tour(&dir, window, &workbench, cx).await {
            log::error!("screenshot tour failed: {e:#}");
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}

/// Draws the window now. The system stops drawing a window that is hidden
/// or not in front, and the tour captures the last drawn frame, so it draws
/// itself rather than wait for the system.
fn draw(window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
}

/// Waits for a step's background work, then until the viewport shows its
/// newest frame (`Viewport::is_current`), drawing as it goes so that the
/// viewport renders and paints its frames.
async fn settle(
    window: AnyWindowHandle,
    workbench: &Entity<Workbench>,
    cx: &mut AsyncApp,
) -> Result<()> {
    let mut waited = Duration::ZERO;
    while waited < SETTLE {
        cx.update_window(window, |_, window, cx| draw(window, cx))?;
        cx.background_executor().timer(POLL).await;
        waited += POLL;
    }
    let mut waited = Duration::ZERO;
    while waited < FRAME_TIMEOUT {
        let current = cx.update_window(window, |_, window, cx| {
            draw(window, cx);
            workbench
                .read(cx)
                .workspace()
                .is_none_or(|workspace| workspace.read(cx).viewport_is_current(cx))
        })?;
        if current {
            break;
        }
        cx.background_executor().timer(POLL).await;
        waited += POLL;
    }
    if waited >= FRAME_TIMEOUT {
        log::warn!("screenshot tour: the viewport frame did not land in time");
    }
    // One more draw with the landed frame, which the capture uses.
    cx.update_window(window, |_, window, cx| draw(window, cx))?;
    Ok(())
}

async fn tour(
    dir: &Path,
    window: AnyWindowHandle,
    workbench: &Entity<Workbench>,
    cx: &mut AsyncApp,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    settle(window, workbench, cx).await?;
    for (name, step) in TOUR {
        cx.update_window(window, |_, window, cx| {
            workbench.update(cx, |workbench, cx| step(workbench, window, cx));
        })?;
        settle(window, workbench, cx).await?;
        if name.starts_with('_') {
            continue;
        }
        let image = cx.update_window(window, |_, window, _| window.render_to_image())??;
        let path = dir.join(format!("{name}.png"));
        image.save(&path)?;
        log::info!("saved {}", path.display());
    }
    Ok(())
}
