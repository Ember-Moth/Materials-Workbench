//! The structure workspace: one open crystal structure with its 3D view,
//! side panels, commands and dialogs (`docs/design/ui-shell.md` §4–§5).
//!
//! Viewing, selecting, measuring, editing and exporting are real: the
//! fields, the element pickers, dragging atoms in the 3D view, adding and
//! deleting sites run the domain commands of `mw_crystal` on the open
//! document, with undo and redo, and the export dialog writes CIF, POSCAR
//! or extended XYZ. Documents come
//! from project files and give back what to save, with their view state.
//! Converting to P1 and changing the cell are commands on the document; a
//! supercell is a new structure the host adds to the project. The display
//! models draw ball and stick, space filling, coordination polyhedra, or
//! displacement ellipsoids; lattice planes can be drawn in any of them.
//! Anything mocked says so where it happens (see `docs/architecture.md`,
//! "Mocks").

rust_i18n::i18n!("locales", fallback = "en");

/// Looks up an application string in this crate's locale files.
macro_rules! tr {
    ($($args:tt)*) => {
        gpui_kit::SharedString::from(rust_i18n::t!($($args)*).to_string())
    };
}

mod actions;
mod cell_dialog;
mod diagnostics;
mod dialogs;
mod display_range;
mod document;
mod editing;
mod ellipsoids;
mod export;
mod format;
pub mod import;
mod measure;
mod moving;
mod overlays;
mod panel;
mod planes;
mod polyhedra;
mod scene;
mod selection;
mod symmetrize_dialog;
#[cfg(test)]
mod ui_tests;
mod workspace;

pub use actions::{
    AddMixedOccupant, AddSite, AlignCell, ClearSelection, ConvertToP1, DeleteSelection,
    EditDisplayRange, EditEllipsoids, EditPlanes, EditPolyhedra, ExportStructure, KEY_CONTEXT,
    MakeSupercell, PaletteCommand, Redo, ResetView, ShowBallAndStick, ShowDiagnosticsPanel,
    ShowEllipsoids, ShowMeasurementsPanel, ShowPolyhedra, ShowPropertiesPanel, ShowSitesPanel,
    ShowSpaceFilling, Symmetrize, ToggleMeasure, ToggleMoveAtoms, TogglePanel, TransformCell, Undo,
    bind_keys, palette_commands,
};
pub use document::StructureDocument;
pub use scene::DisplayModel;
pub use workspace::{StructureWorkspace, StructureWorkspaceEvent, Supercell, supercell_name};
