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

const TOUR: &[(&str, Step)] = &[
    ("01-main", |_, _, _| {}),
    ("02-sites", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowSitesPanel), cx)
    }),
    ("03-palette", |workbench, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowPropertiesPanel), cx);
        workbench.toggle_command_palette(&crate::actions::ToggleCommandPalette, window, cx);
    }),
    ("04-delete-dialog", |_, window, cx| {
        window.close_dialog(cx);
        window.dispatch_action(Box::new(mw_structure::DeleteSelection), cx);
    }),
    ("05-dark", |_, window, cx| {
        window.close_dialog(cx);
        settings::set_dark_mode(true, window, cx);
    }),
    ("06-english", |_, window, cx| {
        settings::set_dark_mode(false, window, cx);
        settings::set_language("en", cx);
    }),
    ("07-srtio3", |workbench, window, cx| {
        settings::set_language("zh-CN", cx);
        workbench.open_structure(StructureEntryId(3), window, cx);
    }),
    ("08-supercell", |workbench, window, cx| {
        workbench.open_structure(StructureEntryId(2), window, cx);
    }),
    // Opening files goes through the same path as the file dialog and drops.
    // Several at once, as when dropping a selection: the first opens, the
    // rest join the "imported files" project.
    ("09-import-files", |workbench, window, cx| {
        let files = [
            "bst_su_aniso.cif",
            "rutile.cif",
            "SrTiO3.cif",
            "multi_block.cif",
        ];
        workbench.import_paths(files.map(fixture).to_vec(), window, cx);
    }),
    // Entry 5 is the imported rutile, after the three samples and BST.
    ("10-imported-rutile", |workbench, window, cx| {
        workbench.open_structure(StructureEntryId(5), window, cx);
    }),
    ("11-import-zincite", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("zincite_legacy.cif")], window, cx);
    }),
    ("12-import-diagnostics", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowDiagnosticsPanel), cx)
    }),
    // Sites snapped onto special positions, rows merged and dropped.
    ("13-import-snapped", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("rutile_near_special.cif")], window, cx);
    }),
    ("14-snapped-diagnostics", |_, window, cx| {
        window.dispatch_action(Box::new(mw_structure::ShowDiagnosticsPanel), cx)
    }),
    ("15-import-failed", |workbench, window, cx| {
        workbench.import_paths(vec![fixture("malformed.cif")], window, cx);
    }),
    ("16-import-errors", |_, window, cx| {
        OpenedFile::read(fixture("malformed.cif")).show_errors(window, cx);
    }),
    ("17-settings", |_, window, cx| {
        window.close_dialog(cx);
        settings::open(window, cx)
    }),
    ("18-no-sidebar", |workbench, window, cx| {
        window.close_dialog(cx);
        workbench.set_sidebar_open(false, cx);
    }),
    ("19-empty", |workbench, window, cx| {
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
