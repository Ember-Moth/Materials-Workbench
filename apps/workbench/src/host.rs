//! The application as the host MCP clients work on
//! (`docs/design/ai-integration.md` §5.1): the projects open in the window
//! and their structures. Calls run on the main thread with the workbench at
//! hand (`mcp.rs`), so what they open appears in the window as the user's
//! own opening would.
//!
//! Read-only for now: capabilities read the open structures' documents but
//! do not get to edit them, since edits must go through the workspace,
//! which updates what it shows (v0.3).

use gpui_kit::*;
use mw_automation::{
    CallError, EntryInfo, Host, Opening, ProjectInfo, Projects, RecentProject, RecentProjects,
    StructureEntryId, StructureInfo, duplicate_project, read_error, unknown_structure,
};
use mw_crystal::CrystalDocument;
use mw_crystal_io::ImportedBlock;
use mw_mcp::Access;
use mw_project_file::ProjectId;
use mw_structure::StructureWorkspace;

use crate::config::AppPaths;
use crate::projects::same_dir;
use crate::workbench::Workbench;

/// The workbench during one call, with what the calling client may open.
pub(crate) struct AppHost<'a, 'c> {
    workbench: &'a mut Workbench,
    access: &'a Access,
    window: &'a mut Window,
    cx: &'a mut Context<'c, Workbench>,
}

impl<'a, 'c> AppHost<'a, 'c> {
    pub fn new(
        workbench: &'a mut Workbench,
        access: &'a Access,
        window: &'a mut Window,
        cx: &'a mut Context<'c, Workbench>,
    ) -> Self {
        Self {
            workbench,
            access,
            window,
            cx,
        }
    }

    fn app(&self) -> &App {
        self.cx
    }

    fn workspace(&self, id: StructureEntryId) -> Option<&StructureWorkspace> {
        let opened = self.workbench.opened.iter().find(|o| o.id == id)?;
        Some(opened.workspace.read(self.app()))
    }

    fn is_open(&self, id: StructureEntryId) -> bool {
        self.workbench.opened.iter().any(|o| o.id == id)
    }
}

impl Host for AppHost<'_, '_> {
    fn structures(&self) -> Vec<StructureInfo> {
        let catalog = self.workbench.catalog(self.app());
        catalog
            .projects()
            .iter()
            .flat_map(|project| {
                project
                    .structures()
                    .iter()
                    .filter(|s| self.is_open(s.id()))
                    .map(|s| StructureInfo::new(s.id(), s.name(), project.name()))
            })
            .collect()
    }

    fn document(&self, id: StructureEntryId) -> Option<&CrystalDocument> {
        Some(self.workspace(id)?.crystal())
    }

    fn document_mut(&mut self, _: StructureEntryId) -> Option<&mut CrystalDocument> {
        None
    }

    fn source(&self, id: StructureEntryId) -> Option<&ImportedBlock> {
        self.workspace(id)?.source()
    }

    fn has_window(&self) -> bool {
        true
    }

    fn projects(&self) -> Option<&dyn Projects> {
        Some(self)
    }

    fn projects_mut(&mut self) -> Option<&mut dyn Projects> {
        Some(self)
    }
}

impl Projects for AppHost<'_, '_> {
    fn projects(&self) -> Vec<ProjectInfo> {
        self.workbench
            .catalog(self.app())
            .projects()
            .iter()
            .map(|p| ProjectInfo::new(p.file_id(), p.name(), p.path().map(Into::into)))
            .collect()
    }

    fn recent_projects(&self) -> RecentProjects {
        let (projects, withheld): (Vec<_>, Vec<_>) = self
            .workbench
            .session
            .recent
            .iter()
            .partition(|recent| self.access.permits(&recent.path));
        RecentProjects {
            projects: projects
                .into_iter()
                .map(|r| RecentProject::new(r.id, &r.name, r.path.clone(), r.opened_at.clone()))
                .collect(),
            withheld: withheld.len(),
        }
    }

    fn entries(&self) -> Vec<EntryInfo> {
        let catalog = self.workbench.catalog(self.app());
        catalog
            .projects()
            .iter()
            .flat_map(|project| {
                project.structures().iter().map(|s| {
                    EntryInfo::new(
                        s.id(),
                        project.file_id(),
                        s.entry().id,
                        s.name(),
                        s.entry().provenance.clone(),
                        self.is_open(s.id()),
                    )
                })
            })
            .collect()
    }

    /// Opens the project in the window, among the others, as the user's
    /// "Open project…" would, without showing it: the client goes on with
    /// `structure.open`.
    fn open_project(&mut self, path: &str) -> Result<(ProjectId, Opening), CallError> {
        let path = self.access.project(path)?;
        let projects = Projects::projects(self);
        let at_path = projects
            .iter()
            .find(|p| p.path.as_deref().is_some_and(|own| same_dir(own, &path)));
        if let Some(open) = at_path {
            return Ok((open.id, Opening::AlreadyOpen));
        }
        let (dir, manifest) = mw_project_file::read_manifest(&path).map_err(read_error)?;
        if let Some(open) = projects.iter().find(|p| p.id == manifest.id) {
            return Err(duplicate_project(&path, open));
        }
        let id = manifest.id;
        let view = dir.read_view().unwrap_or_default();
        self.workbench
            .add_project(&path, dir, manifest, view, self.cx);
        self.workbench.session.save(self.cx.global::<AppPaths>());
        self.workbench.refresh_status(self.cx);
        self.cx.notify();
        Ok((id, Opening::Opened))
    }

    /// Shows the structure in the window, read at once if it is not open.
    fn open_structure(&mut self, id: StructureEntryId) -> Result<Opening, CallError> {
        if self.workbench.catalog(self.app()).structure(id).is_none() {
            return Err(unknown_structure(id));
        }
        if self.is_open(id) {
            return Ok(Opening::AlreadyOpen);
        }
        self.workbench
            .open_structure_now(id, self.window, self.cx)
            .map_err(read_error)?;
        Ok(Opening::Opened)
    }
}
