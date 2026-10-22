//! Development aid: walks through the main screens and renders each to a PNG
//! with `Window::render_to_image`, which needs no screen-recording
//! permission. Enabled by the `screenshot` feature and `MW_SCREENSHOT_DIR`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::WindowExt;
use gpui_kit::*;
use mw_project::StructureEntryId;
use mw_structure::import::OpenedFile;

use crate::workbench::Workbench;
use crate::{palette, settings};

/// Time for the viewport frame to land and the window to redraw.
const SETTLE: Duration = Duration::from_millis(1500);

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
    ("10-supercell", |workbench, window, cx| {
        workbench.open_structure(StructureEntryId(2), window, cx);
    }),
    // Opening files goes through the same path as the file dialog and drops.
    // Several at once, as when dropping a selection: the first opens, the
    // rest join the "imported files" project.
    ("11-import-files", |workbench, window, cx| {
        let files = [
            "bst_su_aniso.cif",
            "rutile.cif",
            "SrTiO3.cif",
            "multi_block.cif",
        ];
        workbench.import_paths(files.map(fixture).to_vec(), window, cx);
    }),
    // Entry 5 is the imported rutile, after the three samples and BST.
    ("12-imported-rutile", |workbench, window, cx| {
        workbench.open_structure(StructureEntryId(5), window, cx);
    }),
    ("13-import-zincite", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("zincite_legacy.cif")], window, cx);
    }),
    ("14-import-diagnostics", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowDiagnosticsPanel), cx)
    }),
    // Sites snapped onto special positions, rows merged and dropped.
    ("15-import-snapped", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("rutile_near_special.cif")], window, cx);
    }),
    ("16-snapped-diagnostics", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowDiagnosticsPanel), cx)
    }),
    ("17-import-failed", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("malformed.cif")], window, cx);
    }),
    ("18-import-errors", |_, window, cx| {
        OpenedFile::read(fixture("malformed.cif")).show_errors(window, cx);
    }),
    ("19-settings", |_, window, cx| {
        window.close_dialog(cx);
        settings::open(window, cx)
    }),
    ("20-no-sidebar", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.set_sidebar_open(false, cx);
    }),
    ("21-empty", |workbench, window, cx| {
        workbench.set_sidebar_open(true, cx);
        workbench.close_structure(window, cx);
    }),
];

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

async fn tour(
    dir: &Path,
    window: AnyWindowHandle,
    workbench: &Entity<Workbench>,
    cx: &mut AsyncApp,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    cx.background_executor().timer(SETTLE).await;
    for (name, step) in TOUR {
        cx.update_window(window, |_, window, cx| {
            workbench.update(cx, |workbench, cx| step(workbench, window, cx));
        })?;
        cx.background_executor().timer(SETTLE).await;
        let image = cx.update_window(window, |_, window, _| window.render_to_image())??;
        let path = dir.join(format!("{name}.png"));
        image.save(&path)?;
        log::info!("saved {}", path.display());
    }
    Ok(())
}
