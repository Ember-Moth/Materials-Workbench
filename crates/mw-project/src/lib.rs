//! Projects: the projects open in this session with their structures, and
//! the sidebar that navigates them. A project is read from and saved to a
//! project directory (`mw-project-file`); structures imported or derived
//! since the last save are held here until then.

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
    DerivedEntry, ImportedEntry, Pending, Project, ProjectCatalog, ProjectId, SaveSnapshot,
    StructureEntry, StructureEntryId,
};
pub use sidebar::{ProjectSidebar, ProjectSidebarEvent};
