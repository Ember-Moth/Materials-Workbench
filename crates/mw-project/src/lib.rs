//! Projects: the catalog of projects and their structures, and the sidebar
//! that navigates it.
//!
//! The catalog is in memory: built-in samples stand in for project files,
//! and opened files are added to it for the session (see
//! `docs/architecture.md`, "Mocks").

rust_i18n::i18n!("locales", fallback = "en");

/// Looks up an application string in this crate's locale files.
macro_rules! tr {
    ($($args:tt)*) => {
        gpui_kit::SharedString::from(rust_i18n::t!($($args)*).to_string())
    };
}

mod catalog;
mod sidebar;

pub use catalog::{
    ImportedEntry, Project, ProjectCatalog, ProjectId, StructureEntry, StructureEntryId,
    StructureOrigin, StructureSource,
};
pub use sidebar::{ProjectSidebar, ProjectSidebarEvent};
