//! The structure workspace: one open crystal structure with its 3D view,
//! side panels, commands and dialogs (`docs/design/ui-shell.md` §4–§5).
//!
//! Viewing, selecting, measuring and editing are real: the fields, the
//! element pickers and the delete command run the domain commands of
//! `mw_crystal` on the open document, with undo and redo. Converting to P1,
//! supercells and exporting are still mocked; each mock says so where it
//! happens (see `docs/architecture.md`, "Mocks").

rust_i18n::i18n!("locales", fallback = "en");

/// Looks up an application string in this crate's locale files.
macro_rules! tr {
    ($($args:tt)*) => {
        gpui_kit::SharedString::from(rust_i18n::t!($($args)*).to_string())
    };
}

mod actions;
mod diagnostics;
mod dialogs;
mod document;
mod editing;
mod format;
pub mod import;
mod measure;
mod overlays;
mod panel;
mod scene;
mod selection;
mod workspace;

pub use actions::{
    ClearSelection, ConvertToP1, DeleteSelection, ExportStructure, KEY_CONTEXT, MakeSupercell,
    PaletteCommand, Redo, ResetView, ShowBallAndStick, ShowDiagnosticsPanel, ShowMeasurementsPanel,
    ShowPolyhedra, ShowPropertiesPanel, ShowSitesPanel, ShowSpaceFilling, ToggleMeasure,
    TogglePanel, Undo, bind_keys, palette_commands,
};
pub use document::StructureDocument;
pub use workspace::StructureWorkspace;
