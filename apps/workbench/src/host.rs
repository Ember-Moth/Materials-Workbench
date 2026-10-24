//! The application as the host MCP clients work on
//! (`docs/design/ai-integration.md` §5.1, §5.3): the projects open in the
//! window and their structures. Calls run on the main thread with the
//! workbench at hand (`mcp.rs`), so what they open or change appears in the
//! window as the user's own would.
//!
//! Edits go through the structure's workspace, onto the same undo stack as
//! the user's, marked as the client's. New structures, imported or derived,
//! join their project unsaved, as the user's would; the user saves. A file
//! is written only once the user confirmed it in the window.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::*;
use mw_automation::{
    Answer, CallError, Client, Confirmation, DisplayModel, EntryInfo, Host, Image,
    MAX_SNAPSHOT_SIZE, MIN_SNAPSHOT_SIZE, OpenedFile, Opening, Permissions, ProjectInfo, Projects,
    RecentProject, RecentProjects, StructureEntryId, StructureInfo, ViewChange, duplicate_project,
    parse_import, read_error, unknown_structure,
};
use mw_crystal::{Agent, Command, CrystalDocument, Outcome, Structure};
use mw_crystal_io::ImportedBlock;
use mw_mcp::Access;
use mw_project::ImportedEntry;
use mw_project_file::{AuditRecord, ProjectId};
use mw_structure::StructureWorkspace;

use crate::config::AppPaths;
use crate::projects::same_dir;
use crate::settings::AiSetting;
use crate::workbench::Workbench;

/// The size of a snapshot the client left to the viewport: the viewport's
/// own, within the sizes a snapshot may have.
fn snapshot_size(viewport: Option<(u32, u32)>) -> (u32, u32) {
    let Some((width, height)) = viewport.filter(|&(w, h)| w > 0 && h > 0) else {
        // Not laid out yet.
        return (1024, 768);
    };
    let scale = (f64::from(MAX_SNAPSHOT_SIZE) / f64::from(width.max(height))).min(1.0);
    let side = |v: u32| {
        ((f64::from(v) * scale).round() as u32).clamp(MIN_SNAPSHOT_SIZE, MAX_SNAPSHOT_SIZE)
    };
    (side(width), side(height))
}

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

    fn workspace_entity(&self, id: StructureEntryId) -> Option<Entity<StructureWorkspace>> {
        let opened = self.workbench.opened.iter().find(|o| o.id == id)?;
        Some(opened.workspace.clone())
    }

    fn workspace(&self, id: StructureEntryId) -> Option<&StructureWorkspace> {
        let opened = self.workbench.opened.iter().find(|o| o.id == id)?;
        Some(opened.workspace.read(self.app()))
    }

    fn is_open(&self, id: StructureEntryId) -> bool {
        self.workbench.opened.iter().any(|o| o.id == id)
    }

    /// The open projects' directories, where clients may read and write as
    /// in the directories they allow (design §7.2).
    fn project_dirs(&self) -> Vec<PathBuf> {
        self.workbench
            .catalog(self.app())
            .projects()
            .iter()
            .filter_map(|p| p.path()?.canonicalize().ok())
            .collect()
    }

    /// Makes the open structure the one the window shows.
    fn show(&mut self, id: StructureEntryId) -> Result<(), CallError> {
        if !self.is_open(id) {
            return Err(unknown_structure(id));
        }
        self.workbench
            .open_structure_now(id, self.window, self.cx)
            .map_err(read_error)
    }

    /// Runs `change` on the open structure's workspace.
    fn on_workspace<T>(
        &mut self,
        id: StructureEntryId,
        change: impl FnOnce(&mut StructureWorkspace, &mut Window, &mut Context<StructureWorkspace>) -> T,
    ) -> Result<T, CallError> {
        let workspace = self
            .workspace_entity(id)
            .ok_or_else(|| unknown_structure(id))?;
        let window = &mut *self.window;
        Ok(workspace.update(self.cx, |workspace, cx| change(workspace, window, cx)))
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

    fn execute(
        &mut self,
        id: StructureEntryId,
        command: Command,
        agent: Option<Agent>,
    ) -> Result<Outcome, CallError> {
        let outcome = self.on_workspace(id, |workspace, window, cx| match agent {
            Some(agent) => workspace.execute_by(command, agent, window, cx),
            None => workspace.execute(command, window, cx),
        })?;
        Ok(outcome?)
    }

    fn undo(
        &mut self,
        id: StructureEntryId,
        agent: Option<Agent>,
    ) -> Result<Option<Outcome>, CallError> {
        self.on_workspace(id, |workspace, window, cx| {
            workspace.undo_by(agent, window, cx)
        })
    }

    fn redo(
        &mut self,
        id: StructureEntryId,
        agent: Option<Agent>,
    ) -> Result<Option<Outcome>, CallError> {
        self.on_workspace(id, |workspace, window, cx| {
            workspace.redo_by(agent, window, cx)
        })
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

    /// Kept with the project until it is saved, closed or the application
    /// quits (`ai_log.rs`); the client joins the settings' list.
    fn audit(&mut self, project: Option<ProjectId>, record: AuditRecord) {
        log::info!("audit: {}", record.to_line());
        let client = Client::new(record.client.name.clone(), record.client.title.clone());
        AiSetting::remember(&client, self.cx);
        if let Some(project) = project {
            self.workbench.keep_audit(project, record, self.cx);
        }
    }

    fn writable(&self, path: &str) -> Result<PathBuf, CallError> {
        self.access.writable(path, &self.project_dirs())
    }

    fn permissions(&self, client: &Client) -> Permissions {
        AiSetting::permissions(client, self.app())
    }

    fn ask(&mut self, client: &Client, question: &Confirmation, answer: Answer) -> bool {
        crate::mcp::ask(client, question, answer, self.window, self.cx);
        true
    }

    /// Shows the structure, then changes what the window shows of it, as
    /// the user's clicks and toolbar would.
    fn change_view(&mut self, id: StructureEntryId, change: ViewChange) -> Result<(), CallError> {
        self.show(id)?;
        self.on_workspace(id, |workspace, window, cx| match change {
            ViewChange::Select(targets) => workspace.select(&targets, window, cx),
            ViewChange::DisplayModel(model) => {
                let model = match model {
                    DisplayModel::BallAndStick => mw_structure::DisplayModel::BallAndStick,
                    DisplayModel::SpaceFilling => mw_structure::DisplayModel::SpaceFilling,
                    DisplayModel::Polyhedra => mw_structure::DisplayModel::Polyhedra,
                };
                workspace.set_display_model(model, window, cx);
            }
            ViewChange::ResetCamera => workspace.reset_camera(cx),
            // Changes of a later version of the registry.
            _ => {}
        })
    }

    /// Shows the structure and renders it with its viewport.
    fn snapshot(
        &mut self,
        id: StructureEntryId,
        width: u32,
        height: u32,
    ) -> Result<Image, CallError> {
        self.show(id)?;
        let snapshot = self.on_workspace(id, |workspace, _, cx| {
            let (width, height) = if width == 0 && height == 0 {
                snapshot_size(workspace.viewport_size(cx))
            } else {
                (width, height)
            };
            workspace.snapshot(width, height, cx)
        })?;
        let snapshot = snapshot.map_err(|reason| {
            CallError::new(
                "render_failed",
                format!("the viewport could not draw the structure: {reason}"),
            )
        })?;
        Ok(Image::new(snapshot.png, snapshot.width, snapshot.height))
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
            .add_project(&path, dir, manifest, view, self.window, self.cx);
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

    /// Imports the file into the project as the user's "Open file…" would:
    /// the structures join it unsaved, open, and the first is shown.
    fn import_file(&mut self, project: ProjectId, path: &str) -> Result<OpenedFile, CallError> {
        let target = self
            .workbench
            .catalog(self.app())
            .projects()
            .iter()
            .find(|p| p.file_id() == project)
            .map(|p| p.id())
            .ok_or_else(|| {
                CallError::new(
                    "unknown_project",
                    format!("no open project has the id {project}; project.list lists them"),
                )
            })?;
        let (path, bytes) = self.access.read_in(path, &self.project_dirs())?;
        let parsed = parse_import(&path, bytes)?;
        let (format, bytes) = (parsed.format, parsed.bytes.clone());
        let entries: Vec<ImportedEntry> = parsed
            .structures
            .into_iter()
            .map(|(name, block)| {
                ImportedEntry::new(name, path.clone(), format, bytes.clone(), Arc::new(block))
            })
            .collect();
        let ids = if entries.is_empty() {
            Vec::new()
        } else {
            self.workbench.sidebar.update(self.cx, |sidebar, cx| {
                sidebar.update_catalog(cx, |catalog| catalog.add_imported(target, entries))
            })
        };
        // Each is opened, so that the client can use it; the first is shown.
        for id in ids.iter().rev() {
            self.workbench
                .open_structure_now(*id, self.window, self.cx)
                .map_err(read_error)?;
        }
        self.workbench.refresh_status(self.cx);
        Ok(OpenedFile::new(
            path,
            ids,
            parsed.skipped,
            parsed.diagnostics,
        ))
    }

    /// Adds the supercell to the project after its source, unsaved, and
    /// shows it, as the supercell dialog does.
    fn add_derived(
        &mut self,
        from: StructureEntryId,
        name: Option<&str>,
        structure: Structure,
        matrix: [[i32; 3]; 3],
    ) -> Result<StructureEntryId, CallError> {
        let source = self
            .workspace(from)
            .map(|workspace| workspace.crystal().snapshot())
            .ok_or_else(|| unknown_structure(from))?;
        let from_name = self
            .workbench
            .catalog(self.app())
            .structure(from)
            .map(|(_, entry)| entry.name().to_string())
            .unwrap_or_default();
        let name = name.map_or_else(
            || mw_structure::supercell_name(&from_name, matrix).to_string(),
            str::to_owned,
        );
        let id = self
            .workbench
            .add_derived_structure(from, name, structure, matrix, &source, self.cx)
            .ok_or_else(|| unknown_structure(from))?;
        self.workbench
            .open_structure_now(id, self.window, self.cx)
            .map_err(read_error)?;
        Ok(id)
    }
}
