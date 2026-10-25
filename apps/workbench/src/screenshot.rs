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
    AtomRef, Axis, CellParam, CellParams, CellPreset, CellValues, Command, DisplayRange, Edit,
    Measured, SiteId, Species, SphereCenter, Target,
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

/// A file from the importer's test fixtures: CIF at the top, POSCAR under
/// `poscar/`.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/mw-crystal-io/tests/fixtures")
        .join(name)
}

/// A pointer action on the atom displayed at a fractional position, or on
/// a point of a lattice plane.
#[derive(Clone, Copy)]
enum Pointer {
    Click([f64; 3]),
    /// Over the point of a plane at these fractional coordinates.
    HoverPoint([f64; 3]),
    /// A click on the point of a plane, as in add mode.
    ClickPoint([f64; 3]),
    ShiftClick([f64; 3]),
    Hover([f64; 3]),
    /// Press on the atom and drag by this many logical pixels.
    Drag([f64; 3], f32, f32),
    /// Press on the atom and drag by this many logical pixels, holding ⌥
    /// if asked, without letting go: a screenshot shows the drag.
    Hold([f64; 3], f32, f32, bool),
    /// Let go where the `Hold` with the same values left the pointer.
    Release([f64; 3], f32, f32, bool),
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
            | Pointer::HoverPoint(at)
            | Pointer::ClickPoint(at)
            | Pointer::ShiftClick(at)
            | Pointer::Hover(at)
            | Pointer::Drag(at, _, _)
            | Pointer::Hold(at, ..)
            | Pointer::Release(at, ..)) = *action;
            let shown = workspace.read(cx);
            let position = match action {
                Pointer::HoverPoint(_) | Pointer::ClickPoint(_) => {
                    shown.fractional_point_position(at, cx)
                }
                _ => shown.displayed_atom_position(at, cx),
            };
            let Some(position) = position else {
                log::warn!("screenshot tour: nothing displayed at {at:?}");
                continue;
            };
            let modifiers = Modifiers {
                shift: matches!(action, Pointer::ShiftClick(_)),
                alt: matches!(action, Pointer::Hold(.., true) | Pointer::Release(.., true)),
                ..Modifiers::default()
            };
            let mut release_at = position;
            match *action {
                // The atom has not moved in the document while held, so
                // its position finds the same point again.
                Pointer::Release(_, dx, dy, _) => {
                    window.dispatch_event(
                        PlatformInput::MouseUp(MouseUpEvent {
                            button: MouseButton::Left,
                            position: position + point(px(dx), px(dy)),
                            modifiers,
                            click_count: 1,
                        }),
                        cx,
                    );
                    continue;
                }
                Pointer::Hold(_, dx, dy, _) => {
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
                    window.dispatch_event(
                        PlatformInput::MouseMove(MouseMoveEvent {
                            position: position + point(px(dx), px(dy)),
                            pressed_button: Some(MouseButton::Left),
                            modifiers,
                        }),
                        cx,
                    );
                    continue;
                }
                Pointer::Hover(_) | Pointer::HoverPoint(_) => {
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
                Pointer::Click(_) | Pointer::ShiftClick(_) | Pointer::ClickPoint(_) => {}
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

/// Adds a site to the open structure, as the add-site dialog does.
fn add_site(
    workbench: &Workbench,
    position: [f64; 3],
    element: &'static str,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(workspace) = workbench.workspace().cloned() {
        workspace.update(cx, |workspace, cx| {
            workspace.add_site_at(position, element, 1.0, window, cx);
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
    // body-center Ti. Atoms are located in the image on screen, so the
    // turned frame must land before pointing at them.
    ("_turn-rutile", |workbench, window, cx| {
        point_at(
            workbench,
            &[Pointer::Drag(TI_CENTER, 44., -19.)],
            window,
            cx,
        );
    }),
    // Click the Ti, add an apical O, hover over the other.
    ("03-select-atoms", |workbench, window, cx| {
        const ACTIONS: &[Pointer] = &[
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
    // The shared site is the perovskite's A site: its label field shows the
    // new name, and so do the panel's title and the sites list.
    ("13-rename-label", |workbench, window, cx| {
        let edit = Edit::RenameLabel {
            site: SiteId(1),
            occupant: "Sr1".into(),
            label: "A1".into(),
        };
        execute(workbench, Command::Edit(edit), window, cx);
    }),
    // Ti1 is taken: refused with a notification, Ba1 keeps its label.
    ("14-rename-refused", |workbench, window, cx| {
        let edit = Edit::RenameLabel {
            site: SiteId(1),
            occupant: "Ba1".into(),
            label: "Ti1".into(),
        };
        execute(workbench, Command::Edit(edit), window, cx);
    }),
    // Ti1 has one atom in the cell, so deleting it deletes the site
    // without asking; a notification names what went.
    ("15-select-ti", |workbench, window, cx| {
        point_at(workbench, &[Pointer::Click([0.5, 0.5, 0.5])], window, cx);
    }),
    ("16-delete-site", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::DeleteSelection), cx)
    }),
    // Undo the deletion, the rename, the occupant and the cell: SrTiO₃ as
    // it was.
    ("17-undo", |_, window, cx| {
        for _ in 0..4 {
            window.dispatch_action(Box::new(mw_structure::Undo), cx);
        }
    }),
    // The delete dialog's default for one O of SrTiO₃: convert to P1, then
    // delete that atom alone, as one step.
    ("18-p1-delete", |workbench, window, cx| {
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
    ("19-supercell", |workbench, window, cx| {
        workbench.open_structure(StructureEntryId(2), window, cx);
    }),
    // ⌘S: the measurements made on rutile are saved, its unsaved mark goes.
    ("20-saved", |workbench, window, cx| {
        workbench.save_active_project(&crate::actions::SaveProject, window, cx);
    }),
    // Opening files goes through the same path as the file dialog and drops.
    // Several at once, as when dropping a selection: the first opens, the
    // rest join the open structure's project, unsaved until ⌘S.
    ("21-import-files", |workbench, window, cx| {
        let files = [
            "bst_su_aniso.cif",
            "rutile.cif",
            "SrTiO3.cif",
            "multi_block.cif",
        ];
        workbench.import_paths(files.map(fixture).to_vec(), window, cx);
    }),
    // Entry 5 is the imported rutile, after the three samples and BST.
    ("22-imported-rutile", |workbench, window, cx| {
        workbench.open_structure(StructureEntryId(5), window, cx);
    }),
    ("23-import-zincite", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("zincite_legacy.cif")], window, cx);
    }),
    ("24-import-diagnostics", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowDiagnosticsPanel), cx)
    }),
    // Sites snapped onto special positions, rows merged and dropped.
    ("25-import-snapped", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("rutile_near_special.cif")], window, cx);
    }),
    ("26-snapped-diagnostics", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowDiagnosticsPanel), cx)
    }),
    ("27-import-failed", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("malformed.cif")], window, cx);
    }),
    ("28-import-errors", |_, window, cx| {
        OpenedFile::read(fixture("malformed.cif")).show_errors(window, cx);
    }),
    // Converting rutile to P1: the dialog, then the six sites it gives.
    ("_show-rutile", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.open_structure(StructureEntryId(1), window, cx);
    }),
    // Move mode: dragging an O moves O1 along its line x, x, 0 with all
    // four of its atoms; the prompt says where it goes.
    ("29-move-mode", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
        window.dispatch_action(Box::new(mw_structure::ToggleMoveAtoms), cx);
        point_at(workbench, &[Pointer::Hover(O_EQUATORIAL)], window, cx);
    }),
    ("30-move-drag", |workbench, window, cx| {
        const HOLD: &[Pointer] = &[Pointer::Hold(O_EQUATORIAL, 40., -30., false)];
        point_at(workbench, HOLD, window, cx);
    }),
    // Let go: one step, and the panel shows O1 with x = y moved.
    ("31-moved", |workbench, window, cx| {
        const RELEASE: &[Pointer] = &[Pointer::Release(O_EQUATORIAL, 40., -30., false)];
        point_at(workbench, RELEASE, window, cx);
    }),
    ("_undo-move", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::Undo), cx);
    }),
    // ⌥ leaves the special position: O1 gets more equivalent atoms, and
    // the prompt says how many.
    ("32-move-leave", |workbench, window, cx| {
        point_at(
            workbench,
            &[Pointer::Hold(O_APICAL, 0., -40., true)],
            window,
            cx,
        );
    }),
    ("33-left", |workbench, window, cx| {
        point_at(
            workbench,
            &[Pointer::Release(O_APICAL, 0., -40., true)],
            window,
            cx,
        );
    }),
    // Dispatched actions run after the step, so the undo goes first on its
    // own.
    ("_undo-leave", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::Undo), cx);
        window.dispatch_action(Box::new(mw_structure::ToggleMoveAtoms), cx);
    }),
    // Typed instead: y of x, x, 0 follows x, so setting it sets x.
    ("34-coordinate-follows", |workbench, window, cx| {
        let edit = Edit::SetCoordinate {
            site: SiteId(2),
            axis: Axis::Y,
            value: 0.32,
        };
        execute(workbench, Command::Edit(edit), window, cx);
    }),
    ("35-add-site-dialog", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::Undo), cx);
        window.dispatch_action(Box::new(mw_structure::ShowSitesPanel), cx);
        window.dispatch_action(Box::new(mw_structure::AddSite), cx);
    }),
    // On an atom of O1: nothing is added, and F is offered as a mixed
    // occupant of O1 instead.
    ("36-add-site-coincident", |workbench, window, cx| {
        window.close_dialog(cx);
        add_site(workbench, [0.6952, 0.6952, 0.0], "F", window, cx);
    }),
    // Typed near 2b, 0, 0, 1/2: snapped onto it, with a note.
    ("37-site-added", |workbench, window, cx| {
        window.close_dialog(cx);
        add_site(workbench, [0.0003, 0.0002, 0.5003], "F", window, cx);
    }),
    ("_undo-add", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::Undo), cx);
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
    }),
    ("38-convert-dialog", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
        window.dispatch_action(Box::new(mw_structure::ConvertToP1), cx);
    }),
    ("39-converted", |workbench, window, cx| {
        window.close_dialog(cx);
        execute(workbench, Command::Edit(Edit::ExpandToP1), window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowSitesPanel), cx);
    }),
    // Back to P4₂/mnm in one undo, then the supercell dialog.
    ("40-supercell-dialog", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::Undo), cx);
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
        window.dispatch_action(Box::new(mw_structure::MakeSupercell), cx);
    }),
    // A new derived structure after rutile's other supercell, opened.
    ("41-supercell-made", |workbench, window, cx| {
        window.close_dialog(cx);
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                workspace.make_supercell_of([3, 3, 1], window, cx)
            });
        }
    }),
    // The imports and the supercell are not saved yet: closing their
    // project asks first.
    ("42-unsaved-dialog", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.close_active_project(&crate::actions::CloseProject, window, cx);
    }),
    ("43-settings", |_, window, cx| {
        window.close_dialog(cx);
        settings::open(window, cx)
    }),
    ("44-no-sidebar", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.set_sidebar_open(false, cx);
    }),
    // The structure from step 25 (entry 10) shares a site between Ti and
    // V, so the formats that list atoms are unavailable and say why.
    ("_show-snapped", |workbench, window, cx| {
        workbench.set_sidebar_open(true, cx);
        workbench.open_structure(StructureEntryId(10), window, cx);
    }),
    ("45-export-dialog", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ExportStructure), cx);
    }),
    // The empty state lists the recent projects.
    ("46-empty", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.close_structure(window, cx);
    }),
    // A new project is untitled and empty until files are opened into it.
    ("47-new-project", |workbench, window, cx| {
        workbench.new_project(&crate::actions::NewProject, window, cx);
    }),
    // A POSCAR joins the new project. Its fcc primitive vectors are not in
    // the standard orientation, so the cell keeps their directions and the
    // properties panel says so under the cell parameters.
    ("48-import-poscar", |workbench, window, cx| {
        // The failed import's notification from step 27 would cover the
        // cell parameters.
        window.clear_notifications(cx);
        let file = fixture("poscar/gaas_primitive_cartesian.vasp");
        workbench.import_paths(vec![file], window, cx);
    }),
    // A CONTCAR: selective dynamics flags and velocities are not imported,
    // and a notification says there are diagnostics.
    ("49-import-contcar", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("poscar/CONTCAR")], window, cx);
    }),
    // Both rows, which the notification would cover.
    ("50-contcar-diagnostics", |_, window, cx| {
        window.clear_notifications(cx);
        window.dispatch_action(Box::new(mw_structure::ShowDiagnosticsPanel), cx)
    }),
    // Corundum, R-3c in hexagonal axes, with 30 atoms in the cell.
    ("_import-corundum", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("corundum_hall.cif")], window, cx);
    }),
    // The cell transform dialog starts on the primitive cell: the
    // rhombohedral one, with its P and a third of the atoms.
    ("51-cell-dialog", |workbench, window, cx| {
        window.clear_notifications(cx);
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
        window.dispatch_action(Box::new(mw_structure::TransformCell), cx);
    }),
    // Confirmed: rhombohedral axes, R-3c:R, ten atoms; one undo goes back.
    ("52-cell-transformed", |workbench, window, cx| {
        window.close_dialog(cx);
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                workspace.transform_cell_to(CellPreset::Primitive, window, cx)
            });
        }
    }),
    // The 2×2×2 rutile supercell is in P1; its atoms have rutile's
    // symmetry.
    ("_show-supercell", |workbench, window, cx| {
        workbench.open_structure(StructureEntryId(2), window, cx);
    }),
    // The symmetrize dialog searches in the background and previews keeping
    // the cell: P4₂/mnm with the supercell's translations as centering,
    // two sites.
    ("53-symmetrize-dialog", |workbench, window, cx| {
        window.clear_notifications(cx);
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
        window.dispatch_action(Box::new(mw_structure::Symmetrize), cx);
    }),
    // Confirmed with standardizing: the rutile cell with six atoms, in
    // symmetry mode; one undo goes back.
    ("54-symmetrized", |workbench, window, cx| {
        window.close_dialog(cx);
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                workspace.symmetrize(None, true, window, cx)
            });
        }
    }),
    // The rutile cell as a careless file gives it: b in other decimals, γ
    // off 90° (RestoreCell sets parameters without the constraints). Under
    // the cell parameters, how far it is off and Align.
    ("55-cell-off-symmetry", |workbench, window, cx| {
        let params = CellParams::new(4.594, 4.5941, 2.959, 90.0, 90.0, 90.02);
        execute(
            workbench,
            Command::Edit(Edit::RestoreCell { params }),
            window,
            cx,
        );
    }),
    // Aligned in one step: b = a, γ = 90°, and the note is gone.
    ("56-cell-aligned", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::AlignCell), cx);
    }),
    // Display ranges (M14), on rutile: the popover shows one cell with its
    // boundary atoms, the default.
    ("_show-rutile-range", |workbench, window, cx| {
        window.clear_notifications(cx);
        workbench.open_structure(StructureEntryId(1), window, cx);
    }),
    ("57-display-range", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::EditDisplayRange), cx);
    }),
    // 3 × 3 × 2 cells with their boundary atoms; the view frames them.
    ("58-cells", |workbench, window, cx| {
        let range = DisplayRange::Cells {
            counts: [3, 3, 2],
            boundary: true,
        };
        set_display_range(workbench, range, window, cx);
    }),
    // A sphere of 6 Å around the body-center Ti: its octahedron, the Ti
    // around it and their O, in the cells around.
    ("59-sphere", |workbench, window, cx| {
        let range = DisplayRange::Sphere {
            center: SphereCenter::Atom(AtomRef::new(SiteId(1), 1, [0; 3])),
            radius: 6.0,
        };
        set_display_range(workbench, range, window, cx);
    }),
    ("60-sphere-popover", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::EditDisplayRange), cx);
    }),
    // 10 × 10 × 10 cells, 6531 atoms, worked out in the background.
    ("61-large-range", |workbench, window, cx| {
        let range = DisplayRange::Cells {
            counts: [10, 10, 10],
            boundary: true,
        };
        set_display_range(workbench, range, window, cx);
    }),
    ("_default-range", |workbench, window, cx| {
        set_display_range(workbench, DisplayRange::default(), window, cx);
    }),
    // Coordination polyhedra (M15), on rutile: a TiO₆ octahedron around
    // every Ti of the cell, the corners' completed with O beyond it. The
    // view is reset to take in the octahedra beyond the cell.
    ("62-polyhedra", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowPolyhedra), cx);
        window.dispatch_action(Box::new(mw_structure::ResetView), cx);
    }),
    // The popover beside the button: Ti chosen, the threefold O not.
    ("63-polyhedron-centers", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::EditPolyhedra), cx);
    }),
    // 3 × 3 × 2 cells, as in step 58: chains of edge-sharing octahedra
    // along c, joined at their corners.
    ("64-polyhedra-cells", |workbench, window, cx| {
        set_polyhedra_open(workbench, false, cx);
        let range = DisplayRange::Cells {
            counts: [3, 3, 2],
            boundary: true,
        };
        set_display_range(workbench, range, window, cx);
    }),
    // The SrTiO₃ imported in step 21 (entry 6): the sample's own was
    // converted to P1 and lost an O in step 18.
    ("_show-srtio3-polyhedra", |workbench, window, cx| {
        set_display_range(workbench, DisplayRange::default(), window, cx);
        workbench.open_structure(StructureEntryId(6), window, cx);
    }),
    // SrTiO₃: the TiO₆ octahedron only; the twelvefold Sr are lone atoms.
    ("65-perovskite-polyhedra", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowPolyhedra), cx);
    }),
    // Zincite, imported once more: ZnO₄ tetrahedra.
    ("_import-zincite-polyhedra", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("zincite_legacy.cif")], window, cx);
    }),
    ("66-tetrahedra", |workbench, window, cx| {
        window.clear_notifications(cx);
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowPolyhedra), cx);
        window.dispatch_action(Box::new(mw_structure::ClearSelection), cx);
    }),
    // 3 × 3 × 2 cells: the corner-sharing tetrahedra of wurtzite.
    ("67-tetrahedra-cells", |workbench, window, cx| {
        let range = DisplayRange::Cells {
            counts: [3, 3, 2],
            boundary: true,
        };
        set_display_range(workbench, range, window, cx);
    }),
    ("68-tetrahedra-dark", |_, window, cx| {
        settings::set_dark_mode(true, window, cx);
    }),
    ("_light-again", |_, window, cx| {
        settings::set_dark_mode(false, window, cx);
    }),
    // MCP clients editing in the window (M18), on rutile.
    ("_show-rutile-ai", |workbench, window, cx| {
        window.clear_notifications(cx);
        workbench.open_structure(StructureEntryId(1), window, cx);
    }),
    // An MCP client's edit (M18): the same undo step, and a notification
    // that names the client.
    ("69-ai-edit", |workbench, window, cx| {
        if let Some(workspace) = workbench.workspace().cloned() {
            let values = mw_crystal::CellValues::default().with(mw_crystal::CellParam::C, 3.0);
            let edit = Command::Edit(Edit::SetCell { values });
            let claude = mw_crystal::Agent::new("Claude Code");
            workspace.update(cx, |workspace, cx| {
                workspace.execute_by(edit, claude, window, cx).ok();
            });
        }
    }),
    // Writing a file waits for the user in the window.
    ("70-ai-confirm", |_, window, cx| {
        window.clear_notifications(cx);
        let client = mw_automation::Client::new("claude-code", Some("Claude Code".into()));
        let question = mw_automation::Confirmation::WriteFile {
            structure: "金红石".into(),
            format: "POSCAR".into(),
            path: PathBuf::from("/Users/me/dft/rutile/POSCAR"),
            bytes: 342,
            replaces: false,
        };
        crate::mcp::ask(&client, &question, Box::new(|_| {}), window, cx);
    }),
    // The client is remembered: the settings list it with its own
    // permissions.
    ("71-settings-ai-clients", |_, window, cx| {
        window.close_dialog(cx);
        let client = mw_automation::Client::new("claude-code", Some("Claude Code".into()));
        settings::AiSetting::remember(&client, cx);
        settings::open(window, cx)
    }),
    // Displacement ellipsoids (M16): (Ba,Sr)TiO₃ with anisotropic O.
    ("_import-bst", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.import_paths(vec![fixture("bst_su_aniso.cif")], window, cx);
    }),
    // Each O ellipsoid is long along its own Ti–O–Ti line, with the
    // principal ellipses drawn; Ba/Sr and Ti are round.
    ("72-ellipsoids", |workbench, window, cx| {
        window.clear_notifications(cx);
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowEllipsoids), cx);
        window.dispatch_action(Box::new(mw_structure::ClearSelection), cx);
    }),
    // The popover beside the button, at 90 %: larger ellipsoids.
    ("73-ellipsoid-probability", |workbench, window, cx| {
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                workspace.set_ellipsoid_probability(0.9, cx)
            });
        }
        window.dispatch_action(Box::new(mw_structure::EditEllipsoids), cx);
    }),
    // A POSCAR has no displacement parameters: small spheres, and the
    // info chip says how many sites have no ellipsoid.
    ("_import-poscar-ellipsoids", |workbench, window, cx| {
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                workspace.set_ellipsoids_open(false, window, cx)
            });
        }
        workbench.import_paths(
            vec![fixture("poscar/gaas_primitive_cartesian.vasp")],
            window,
            cx,
        );
    }),
    ("74-ellipsoids-without-adp", |workbench, window, cx| {
        window.clear_notifications(cx);
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowEllipsoids), cx);
        window.dispatch_action(Box::new(mw_structure::ClearSelection), cx);
    }),
    // Lattice planes (M16), on rutile.
    ("_show-rutile-planes", |workbench, window, cx| {
        window.clear_notifications(cx);
        workbench.open_structure(StructureEntryId(1), window, cx);
    }),
    // (1 1 0) through the cell's diagonal and (0 0 1) halfway up, each in
    // its color; the popover lists them with their spacings.
    ("75-planes", |workbench, window, cx| {
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowBallAndStick), cx);
        window.dispatch_action(Box::new(mw_structure::ClearSelection), cx);
        add_plane(workbench, [1, 1, 0], 1.0, window, cx);
        add_plane(workbench, [0, 0, 1], 0.5, window, cx);
        window.dispatch_action(Box::new(mw_structure::EditPlanes), cx);
    }),
    // Planes span the display range and sort among the polyhedra: 2 × 2 × 2
    // cells of octahedra, and (1 1 0) two spacings out through the block's
    // middle.
    ("76-planes-polyhedra", |workbench, window, cx| {
        set_planes_open(workbench, false, window, cx);
        add_plane(workbench, [1, 1, 0], 2.0, window, cx);
        window.dispatch_action(Box::new(mw_structure::ShowPolyhedra), cx);
        let range = DisplayRange::Cells {
            counts: [2, 2, 2],
            boundary: true,
        };
        set_display_range(workbench, range, window, cx);
    }),
    // In a sphere around the body-center Ti, the planes are discs.
    ("77-planes-sphere", |workbench, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowBallAndStick), cx);
        let range = DisplayRange::Sphere {
            center: SphereCenter::Point([0.5; 3]),
            radius: 6.0,
        };
        set_display_range(workbench, range, window, cx);
    }),
    // Add mode (M17): rutile's cell with the mirror plane z = c/2 only.
    ("_m17-rutile-plane", |workbench, window, cx| {
        let range = DisplayRange::default();
        set_display_range(workbench, range, window, cx);
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                workspace.set_view(&mw_project_file::EntryView::default(), cx);
                workspace.reset_camera(cx);
            });
        }
        add_plane(workbench, [0, 0, 1], 0.5, window, cx);
        set_planes_open(workbench, false, window, cx);
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ClearSelection), cx);
    }),
    // The prompt bar with its element picker, and over the plane where an
    // O would go: on the mirror, x, y, 1/2, eight atoms.
    ("78-add-mode", |workbench, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ToggleAddOnPlanes), cx);
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                workspace.set_add_element("O", window, cx)
            });
        }
        point_at(workbench, &[Pointer::HoverPoint(PLANE_POINT)], window, cx);
    }),
    // A click adds it there, selected: the panel shows O2 on the mirror.
    ("79-added-on-plane", |workbench, window, cx| {
        point_at(workbench, &[Pointer::ClickPoint(PLANE_POINT)], window, cx);
    }),
    // The disorder fields (M17) for O1 and O2 selected together: O2 is in
    // group 1 of assembly A and O1 is not, so the fields show no value.
    ("80-disorder-fields", |workbench, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ToggleAddOnPlanes), cx);
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                let disorder = mw_crystal::Disorder::new(Some("A".into()), "1".into());
                workspace.set_disorder(&[SiteId(3)], Some(disorder), window, cx);
                let sites = [Target::Site(SiteId(2)), Target::Site(SiteId(3))];
                workspace.select(&sites, window, cx);
            });
        }
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
    }),
    // O1 in group 2: the list marks both and has both selected.
    ("81-disorder-sites", |workbench, window, cx| {
        window.clear_notifications(cx);
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                let disorder = mw_crystal::Disorder::new(Some("A".into()), "2".into());
                workspace.set_disorder(&[SiteId(2)], Some(disorder), window, cx);
            });
        }
        window.dispatch_action(Box::new(mw_structure::ShowSitesPanel), cx);
    }),
    // Back as rutile was; then a file with a disordered ethyl group.
    ("_m17-undo-and-import", |workbench, window, cx| {
        window.clear_notifications(cx);
        focus_workspace(workbench, window, cx);
        for _ in 0..3 {
            window.dispatch_action(Box::new(mw_structure::Undo), cx);
        }
        workbench.import_paths(vec![fixture("zero_occupancy.cif")], window, cx);
    }),
    // Both components of the ethyl group overlap; the popover offers
    // either.
    ("82-disorder-all", |workbench, window, cx| {
        window.clear_notifications(cx);
        focus_workspace(workbench, window, cx);
        window.dispatch_action(Box::new(mw_structure::ClearSelection), cx);
        window.dispatch_action(Box::new(mw_structure::EditDisorderGroups), cx);
    }),
    // Group 1 only: C5 and C6.
    ("83-disorder-group", |workbench, _, cx| {
        if let Some(workspace) = workbench.workspace().cloned() {
            workspace.update(cx, |workspace, cx| {
                workspace.show_disorder_group(Some("A"), Some("1"), cx)
            });
        }
    }),
];

/// A point of rutile's mirror plane z = c/2 away from the atoms on it.
const PLANE_POINT: [f64; 3] = [0.3, 0.1, 0.5];

/// Draws a lattice plane on the open structure, as the planes popover does.
fn add_plane(
    workbench: &Workbench,
    miller: [i32; 3],
    position: f64,
    window: &mut Window,
    cx: &mut App,
) {
    let Ok(plane) = mw_crystal::LatticePlane::new(miller, position) else {
        return;
    };
    if let Some(workspace) = workbench.workspace().cloned() {
        workspace.update(cx, |workspace, cx| workspace.add_plane(plane, window, cx));
    }
}

/// Opens or closes the lattice planes popover.
fn set_planes_open(workbench: &Workbench, open: bool, window: &mut Window, cx: &mut App) {
    if let Some(workspace) = workbench.workspace().cloned() {
        workspace.update(cx, |workspace, cx| {
            workspace.set_planes_open(open, window, cx)
        });
    }
}

/// Opens or closes the popover choosing the polyhedra's centers.
fn set_polyhedra_open(workbench: &Workbench, open: bool, cx: &mut App) {
    if let Some(workspace) = workbench.workspace().cloned() {
        workspace.update(cx, |workspace, cx| workspace.set_polyhedra_open(open, cx));
    }
}

/// Closes the display range popover and shows `range`, as its fields do.
fn set_display_range(
    workbench: &Workbench,
    range: DisplayRange,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(workspace) = workbench.workspace().cloned() {
        workspace.update(cx, |workspace, cx| {
            workspace.set_range_open(false, window, cx);
            if let Err(error) = workspace.set_display_range(range, cx) {
                log::warn!("screenshot tour: {error}");
            }
        });
    }
}

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
