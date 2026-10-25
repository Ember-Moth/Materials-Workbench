use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::command::CommandState;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable, StyledExt, WindowExt, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal_io::ImportedBlock;
use mw_project::{
    DerivedEntry, ImportedEntry, ProjectCatalog, ProjectId, ProjectSidebar, ProjectSidebarEvent,
    StructureEntryId,
};
use mw_project_file::{
    AuditRecord, EntryId, EntryView, Operation, ProjectDir, ProjectLock, Provenance, ReadError,
    Sha256, StructureFile, structure_sha256,
};
use mw_structure::import::OpenedFile;
use mw_structure::{StructureDocument, StructureWorkspace, StructureWorkspaceEvent, Supercell};

use crate::actions::*;
use crate::config::{AppPaths, Session};
use crate::settings::McpSetting;
use crate::{palette, settings};

/// A structure opened in this session. Kept while its project is open, so
/// that switching back finds it as it was left: edits, undo history and
/// view.
pub(crate) struct Opened {
    pub id: StructureEntryId,
    pub workspace: Entity<StructureWorkspace>,
    _subscriptions: [Subscription; 2],
}

/// A structure to read from its project before it can be shown.
struct ToRead {
    dir: ProjectDir,
    entry: EntryId,
    provenance: Provenance,
    project_name: String,
    name: String,
    view: Option<EntryView>,
}

/// The main window: project sidebar on the window background, and the open
/// structure (or an empty state) on a card beside it. Owns which structure
/// is open, routes window-wide commands, and opens, saves and closes
/// projects (`projects.rs`).
pub struct Workbench {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) sidebar: Entity<ProjectSidebar>,
    sidebar_open: bool,
    pub(crate) opened: Vec<Opened>,
    /// The structure shown, if any.
    pub(crate) current: Option<StructureEntryId>,
    /// Where opened files go: the project last opened, created or shown.
    pub(crate) active_project: Option<ProjectId>,
    /// The structure last shown in each project, for its view state.
    pub(crate) last_shown: HashMap<ProjectId, StructureEntryId>,
    /// The structure being read from its project, to show when it arrives.
    opening: Option<StructureEntryId>,
    /// While the projects the last session left open are being opened: the
    /// files and folders asked for meanwhile, to open after them.
    starting: Option<Vec<PathBuf>>,
    pub(crate) session: Session,
    pub(crate) palette: Entity<CommandState>,
    /// Listening for MCP clients, while the setting is on (`mcp.rs`).
    pub(crate) mcp: Option<crate::mcp::Serving>,
    /// Records of MCP clients' calls, until they are appended to their
    /// project's audit log (`ai_log.rs`).
    pub(crate) audit: HashMap<ProjectId, Vec<AuditRecord>>,
    /// The open projects this process writes (`projects.rs`,
    /// `docs/design/project-format.md` §15).
    pub(crate) locks: HashMap<ProjectId, ProjectLock>,
    /// For projects another process held when they were opened: their
    /// manifest's digest then, to tell whether it saved them since.
    pub(crate) unlocked: HashMap<ProjectId, Option<Sha256>>,
    _subscriptions: Vec<Subscription>,
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|_| ProjectSidebar::new(ProjectCatalog::new()));
        let subscriptions = vec![
            cx.subscribe_in(&sidebar, window, Self::on_sidebar_event),
            cx.on_app_quit(|this, cx| {
                this.persist(cx);
                // Takes the socket and the token away before the process ends.
                this.mcp = None;
                async {}
            }),
            cx.observe_global_in::<McpSetting>(window, Self::sync_mcp),
        ];
        let session = Session::load(cx.global::<AppPaths>());
        let first_start = session.is_none();
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            sidebar,
            sidebar_open: true,
            opened: Vec::new(),
            current: None,
            active_project: None,
            last_shown: HashMap::new(),
            opening: None,
            starting: Some(Vec::new()),
            session: session.unwrap_or_default(),
            palette: cx.new(|cx| CommandState::new(window, cx)),
            mcp: None,
            audit: HashMap::new(),
            locks: HashMap::new(),
            unlocked: HashMap::new(),
            _subscriptions: subscriptions,
        };
        this.focus_handle.focus(window, cx);
        // Once the window is there: a failure is shown as a notification.
        cx.defer_in(window, Self::sync_mcp);
        let restoring = if first_start {
            this.installing_samples(window, cx)
        } else {
            let open = this.session.open.clone();
            let active = this.session.active.clone();
            this.opening_projects(open, active, window, cx)
        };
        cx.spawn_in(window, async move |this, cx| {
            restoring.await;
            this.update_in(cx, |this, window, cx| {
                let requested = this.starting.take().unwrap_or_default();
                this.import_paths(requested, window, cx);
            })
            .ok();
        })
        .detach();
        this
    }

    /// Opens files and folders the system asks to open (`main.rs`): the
    /// application's arguments, and on macOS what is double-clicked in the
    /// Finder or dropped on the Dock icon. While starting, after the projects
    /// of the last session, so that what was asked for is what is shown.
    pub(crate) fn open_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &mut self.starting {
            Some(requested) => requested.extend(paths),
            None => self.import_paths(paths, window, cx),
        }
    }

    pub(crate) fn catalog<'a>(&self, cx: &'a App) -> &'a ProjectCatalog {
        self.sidebar.read(cx).catalog()
    }

    /// The open structure's workspace, if any.
    pub(crate) fn workspace(&self) -> Option<&Entity<StructureWorkspace>> {
        let current = self.current?;
        self.opened
            .iter()
            .find(|o| o.id == current)
            .map(|o| &o.workspace)
    }

    /// Shows a structure: as it was left if it was opened before, else read
    /// from its project in the background.
    pub(crate) fn open_structure(
        &mut self,
        id: StructureEntryId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(to_read) = self.begin_showing(id, window, cx) else {
            return;
        };
        self.opening = Some(id);
        let ToRead {
            dir,
            entry,
            provenance,
            project_name,
            name,
            view,
        } = to_read;
        let read = cx.background_spawn(async move { read_structure(&dir, entry, &provenance) });
        cx.spawn_in(window, async move |this, cx| {
            let result = read.await;
            this.update_in(cx, |this, window, cx| {
                // The user may have gone on to another structure meanwhile,
                // or closed its project.
                if this.opening != Some(id) || this.catalog(cx).structure(id).is_none() {
                    return;
                }
                this.opening = None;
                match result {
                    Ok((file, provenance, block)) => {
                        let document =
                            StructureDocument::new(project_name, name, file, provenance, block);
                        this.show_document(id, document, view, window, cx);
                    }
                    Err(error) => window.push_notification(
                        Notification::error(tr!(
                            "open.structure_failed",
                            name = name,
                            reason = read_error(&error).to_string()
                        )),
                        cx,
                    ),
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shows a structure as [`Self::open_structure`] does, but reads it on
    /// this thread, so that it is open when this returns: for MCP clients,
    /// whose next call uses it (`host.rs`).
    pub(crate) fn open_structure_now(
        &mut self,
        id: StructureEntryId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> std::result::Result<(), ReadError> {
        let Some(to_read) = self.begin_showing(id, window, cx) else {
            return Ok(());
        };
        let (file, provenance, block) =
            read_structure(&to_read.dir, to_read.entry, &to_read.provenance)?;
        // A read of the same structure the user started is done now; one of
        // another stays theirs and shows its structure when it arrives.
        if self.opening == Some(id) {
            self.opening = None;
        }
        let document =
            StructureDocument::new(to_read.project_name, to_read.name, file, provenance, block);
        self.show_document(id, document, to_read.view, window, cx);
        Ok(())
    }

    /// Makes a structure the one shown: at once if it was opened before or
    /// is not on disk yet; otherwise says what to read first.
    fn begin_showing(
        &mut self,
        id: StructureEntryId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ToRead> {
        let (project, entry) = self.catalog(cx).structure(id)?;
        let project_id = project.id();
        let project_name = project.name().to_string();
        let name = entry.name().to_string();
        let provenance = entry.entry().provenance.clone();
        let entry_id = entry.entry().id;
        let view = project.view().entries.get(&entry_id).cloned();
        let pending = entry.pending().cloned();
        let dir = project.dir().cloned();

        self.active_project = Some(project_id);
        self.last_shown.insert(project_id, id);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_selected(Some(id), cx));
        if let Some(opened) = self.opened.iter().find(|o| o.id == id) {
            opened.workspace.focus_handle(cx).focus(window, cx);
            self.current = Some(id);
            self.opening = None;
            cx.notify();
            return None;
        }
        if let Some(pending) = pending {
            // A structure still being read is not to replace this one.
            self.opening = None;
            let file = StructureFile::clone(&pending.file);
            let document =
                StructureDocument::new(project_name, name, file, provenance, pending.block);
            self.show_document(id, document, view, window, cx);
            return None;
        }
        Some(ToRead {
            dir: dir?,
            entry: entry_id,
            provenance,
            project_name,
            name,
            view,
        })
    }

    fn show_document(
        &mut self,
        id: StructureEntryId,
        document: StructureDocument,
        view: Option<EntryView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = cx.new(|cx| StructureWorkspace::new(document, window, cx));
        if let Some(view) = view {
            workspace.update(cx, |workspace, cx| workspace.set_view(&view, cx));
        }
        workspace.focus_handle(cx).focus(window, cx);
        let subscriptions = [
            // Edits change what the header and the sidebar say about saving.
            cx.observe(&workspace, |this, _, cx| this.refresh_status(cx)),
            cx.subscribe_in(&workspace, window, move |this, _, event, window, cx| {
                this.on_workspace_event(id, event, window, cx)
            }),
        ];
        self.opened.push(Opened {
            id,
            workspace,
            _subscriptions: subscriptions,
        });
        self.current = Some(id);
        self.refresh_status(cx);
        cx.notify();
    }

    fn on_workspace_event(
        &mut self,
        from: StructureEntryId,
        event: &StructureWorkspaceEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            StructureWorkspaceEvent::MadeSupercell(supercell) => {
                self.add_supercell(from, supercell, window, cx)
            }
        }
    }

    /// Adds a supercell to the project of the structure it was made from,
    /// after it, as a derived structure (`docs/design/project-format.md`
    /// §13.2), and opens it.
    fn add_supercell(
        &mut self,
        from: StructureEntryId,
        supercell: &Supercell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let added = self.add_derived_structure(
            from,
            supercell.name.to_string(),
            supercell.structure.clone(),
            supercell.matrix,
            &supercell.source,
            cx,
        );
        if let Some(id) = added {
            self.open_structure(id, window, cx);
        }
    }

    /// Adds `structure`, the supercell by `matrix` of `from` as it is now
    /// (`source`), to `from`'s project after it, under `name` or, when that
    /// is taken, `name 2`, `name 3`, …; unsaved. For the supercell dialog
    /// and for MCP clients (`host.rs`).
    pub(crate) fn add_derived_structure(
        &mut self,
        from: StructureEntryId,
        name: String,
        structure: mw_crystal::Structure,
        matrix: [[i32; 3]; 3],
        source: &mw_crystal::Structure,
        cx: &mut Context<Self>,
    ) -> Option<StructureEntryId> {
        let (project, _) = self.catalog(cx).structure(from)?;
        let names: Vec<&str> = project.structures().iter().map(|s| s.name()).collect();
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    name.clone()
                } else {
                    format!("{name} {n}")
                }
            })
            .find(|name| !names.contains(&name.as_str()))
            .expect("some name is free");
        let derived = DerivedEntry::new(name, structure, Operation::MakeSupercell { matrix });
        let added = self.sidebar.update(cx, |sidebar, cx| {
            sidebar.update_catalog(cx, |catalog| catalog.add_derived(from, source, derived))
        });
        self.refresh_status(cx);
        added
    }

    /// Tells the sidebar which structures have unsaved edits, and each open
    /// workspace whether its project has unsaved changes.
    pub(crate) fn refresh_status(&mut self, cx: &mut Context<Self>) {
        let modified: HashSet<StructureEntryId> = self
            .opened
            .iter()
            .filter(|o| o.workspace.read(cx).is_modified())
            .map(|o| o.id)
            .collect();
        let mut updates = Vec::new();
        for project in self.catalog(cx).projects() {
            let project_modified = project.is_modified()
                || project
                    .structures()
                    .iter()
                    .any(|s| modified.contains(&s.id()));
            let name = SharedString::from(project.name().to_string());
            for structure in project.structures() {
                if let Some(opened) = self.opened.iter().find(|o| o.id == structure.id()) {
                    updates.push((opened.workspace.clone(), name.clone(), project_modified));
                }
            }
        }
        for (workspace, name, project_modified) in updates {
            workspace.update(cx, |workspace, cx| {
                workspace.set_project(name, project_modified, cx)
            });
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_modified(modified, cx));
    }

    /// Whether a project has unsaved changes, in itself or in its open
    /// structures.
    pub(crate) fn is_project_modified(&self, project: ProjectId, cx: &App) -> bool {
        let Some(project) = self.catalog(cx).project(project) else {
            return false;
        };
        project.is_modified()
            || project.structures().iter().any(|s| {
                self.opened
                    .iter()
                    .any(|o| o.id == s.id() && o.workspace.read(cx).is_modified())
            })
    }

    pub(crate) fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.sidebar_open = open;
        cx.notify();
    }

    /// Opens what the paths name: project directories as projects, other
    /// files imported into the active project, which is the first of the
    /// projects among the paths if there are any. Reads in the background.
    pub(crate) fn import_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (projects, files): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|path| is_project(path));
        let activate = projects.first().cloned();
        let opening = self.opening_projects(projects, activate, window, cx);
        if files.is_empty() {
            opening.detach();
            return;
        }
        let read = cx.background_spawn(async move {
            files.into_iter().map(OpenedFile::read).collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let files = read.await;
            // Into the project opened with them, if any.
            opening.await;
            this.update_in(cx, |this, window, cx| {
                this.add_opened_files(files, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn add_opened_files(
        &mut self,
        files: Vec<OpenedFile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries: Vec<ImportedEntry> = files
            .iter()
            .flat_map(|file| {
                let bytes = file.bytes();
                file.structures()
                    .into_iter()
                    .filter_map(move |(name, block)| {
                        Some(ImportedEntry::new(
                            name,
                            file.path().to_path_buf(),
                            file.format()?,
                            bytes.clone()?,
                            block,
                        ))
                    })
            })
            .collect();
        if !entries.is_empty() {
            let project = self
                .active_project
                .filter(|id| self.catalog(cx).project(*id).is_some())
                .unwrap_or_else(|| self.create_project(cx));
            let ids = self.sidebar.update(cx, |sidebar, cx| {
                sidebar.update_catalog(cx, |catalog| catalog.add_imported(project, entries))
            });
            if let Some(id) = ids.first() {
                self.open_structure(*id, window, cx);
            }
            self.refresh_status(cx);
        }
        // After opening, so the notifications are not lost behind a focus
        // change and refer to what is now on screen.
        for file in &files {
            file.notify(window, cx);
        }
    }

    /// Shows the empty state; the screenshot tour needs this.
    #[cfg(feature = "screenshot")]
    pub(crate) fn close_structure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.current = None;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_selected(None, cx));
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn on_sidebar_event(
        &mut self,
        _: &Entity<ProjectSidebar>,
        event: &ProjectSidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ProjectSidebarEvent::OpenStructure(id) => self.open_structure(*id, window, cx),
            ProjectSidebarEvent::NewProject => self.new_project(&NewProject, window, cx),
            ProjectSidebarEvent::OpenProject => self.open_project(&OpenProject, window, cx),
            ProjectSidebarEvent::OpenFile => self.open_file(&OpenFile, window, cx),
            ProjectSidebarEvent::SearchCommands => {
                self.toggle_command_palette(&ToggleCommandPalette, window, cx)
            }
            ProjectSidebarEvent::OpenSettings => self.open_settings(&OpenSettings, window, cx),
            ProjectSidebarEvent::Hide => self.toggle_sidebar(&ToggleSidebar, window, cx),
            ProjectSidebarEvent::SaveProject(id) => {
                self.save_project(*id, false, window, cx).detach()
            }
            ProjectSidebarEvent::SaveProjectAs(id) => {
                self.save_project(*id, true, window, cx).detach()
            }
            ProjectSidebarEvent::RevealProject(id) => {
                if let Some(path) = self.catalog(cx).project(*id).and_then(|p| p.path()) {
                    cx.reveal_path(path);
                }
            }
            ProjectSidebarEvent::CloseProject(id) => self.close_project(*id, window, cx),
        }
    }

    // MARK: Actions

    pub(crate) fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
            return;
        }
        let recent = self
            .session
            .recent
            .iter()
            .map(|recent| (SharedString::from(recent.name.clone()), recent.path.clone()))
            .collect();
        let this = cx.entity().downgrade();
        let open_recent =
            std::rc::Rc::new(move |path: PathBuf, window: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| {
                    this.open_projects(vec![path.clone()], Some(path), window, cx)
                })
                .ok();
            });
        palette::open(
            self.palette.clone(),
            self.current.is_some(),
            recent,
            open_recent,
            window,
            cx,
        );
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.set_sidebar_open(!self.sidebar_open, cx);
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(paths) = prompt_result(paths.await, cx) else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.import_paths(paths, window, cx))
                .ok();
        })
        .detach();
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        settings::open(window, cx);
    }

    fn toggle_dark_mode(
        &mut self,
        _: &ToggleDarkMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        settings::toggle_dark_mode(window, cx);
    }

    fn switch_language(&mut self, _: &SwitchLanguage, _: &mut Window, cx: &mut Context<Self>) {
        settings::toggle_language(cx);
    }

    // MARK: Rendering

    fn render_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let recent: Vec<_> = self.session.recent.iter().take(5).cloned().collect();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .p_8()
            .child(
                div()
                    .p_3()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::Inbox).large()),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .max_w_96()
                    .child(div().text_base().font_semibold().child(tr!("empty.title")))
                    .child(
                        div()
                            .text_sm()
                            .text_center()
                            .text_color(theme.muted_foreground)
                            .child(tr!("empty.description")),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("empty-open-file")
                            .label(tr!("empty.open_file"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_file(&OpenFile, window, cx)
                            })),
                    )
                    .child(
                        Button::new("empty-open-samples")
                            .ghost()
                            .label(tr!("empty.open_samples"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_samples(window, cx)),
                            ),
                    ),
            )
            .when(!recent.is_empty(), |this| {
                this.child(
                    v_flex()
                        .w_96()
                        .gap_1()
                        .child(
                            div()
                                .px_2()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(tr!("empty.recent")),
                        )
                        .children(recent.into_iter().map(|recent| {
                            let path = recent.path.clone();
                            ListItem::new(SharedString::from(format!(
                                "recent-{}",
                                recent.path.display()
                            )))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_projects(
                                    vec![path.clone()],
                                    Some(path.clone()),
                                    window,
                                    cx,
                                )
                            }))
                            .child(
                                v_flex()
                                    .w_full()
                                    .min_w_0()
                                    .child(div().text_sm().child(recent.name.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .truncate()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(recent.path.display().to_string()),
                                    ),
                            )
                        })),
                )
            })
    }
}

impl Render for Workbench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.workspace() {
            Some(workspace) => workspace.clone().into_any_element(),
            None => self.render_empty_state(cx).into_any_element(),
        };
        let theme = cx.theme();

        h_flex()
            .id("workbench")
            .key_context("Workbench")
            .track_focus(&self.focus_handle)
            .size_full()
            .items_stretch()
            .bg(theme.sidebar)
            .text_color(theme.foreground)
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::new_project))
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::save_active_project))
            .on_action(cx.listener(Self::save_active_project_as))
            .on_action(cx.listener(Self::close_active_project))
            .on_action(cx.listener(Self::open_sample_projects))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::export_ai_log))
            .on_action(cx.listener(Self::toggle_dark_mode))
            .on_action(cx.listener(Self::switch_language))
            // Files and project folders dropped anywhere on the window are
            // opened.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.import_paths(paths.paths().to_vec(), window, cx)
            }))
            .when(self.sidebar_open, |this| this.child(self.sidebar.clone()))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .p_2()
                    .when(!self.sidebar_open, |this| {
                        // Room for the traffic lights, and the way back.
                        this.child(
                            h_flex().h_8().flex_none().pl_20().child(
                                Button::new("show-sidebar")
                                    .ghost()
                                    .small()
                                    .icon(IconName::PanelLeft)
                                    .tooltip_with_action(tr!("sidebar.show"), &ToggleSidebar, None)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_sidebar(&ToggleSidebar, window, cx)
                                    })),
                            ),
                        )
                    })
                    .child(
                        div()
                            .id("workbench-content")
                            .flex_1()
                            .min_h_0()
                            .rounded(theme.radius_lg)
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .overflow_hidden()
                            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                                style
                                    .border_color(cx.theme().drag_border)
                                    .bg(cx.theme().drop_target)
                            })
                            .child(content),
                    ),
            )
    }
}

/// Whether a path names a project directory rather than a file to import.
/// A missing `.mwproj` counts as a project, so that what is said about it is
/// that the project is not there.
pub(crate) fn is_project(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|e| e == mw_project_file::EXTENSION)
        || (path.is_dir() && path.join(mw_project_file::MANIFEST).exists())
}

/// Reads a structure of a project and, for an imported one, the data block
/// it came from again; a missing or changed copy is shown as such, not an
/// error. Blocks on file I/O.
fn read_structure(
    dir: &ProjectDir,
    entry: EntryId,
    provenance: &Provenance,
) -> Result<(StructureFile, Provenance, Option<Arc<ImportedBlock>>), ReadError> {
    let file = dir.read_structure(entry)?;
    let block = provenance.import().and_then(|import| {
        let bytes = dir.read_source(import).ok()?;
        let mut block = import.read_block(&bytes)?;
        // Whether the structure is still the one imported is the project's
        // to say, by hash (project format §7.1); the block says the same,
        // for capabilities that export it (`mw_automation`), as the MCP
        // server without a window reads it.
        block.structure = (structure_sha256(&file.structure) == import.structure_sha256)
            .then(|| file.structure.clone());
        Some(Arc::new(block))
    });
    Ok((file, provenance.clone(), block))
}

/// Why a project or one of its parts could not be read, in the interface
/// language.
pub(crate) fn read_error(error: &ReadError) -> SharedString {
    match error {
        ReadError::NotAProject(path) => {
            tr!("open.not_a_project", path = path.display().to_string())
        }
        ReadError::Newer { version } => tr!("open.newer", version = version),
        ReadError::Io { path, error } => tr!(
            "open.io",
            path = path.display().to_string(),
            reason = error.to_string()
        ),
        ReadError::Corrupt { path, reason } => tr!(
            "open.corrupt",
            path = path.display().to_string(),
            reason = reason.clone()
        ),
        ReadError::Invalid { path, error } => tr!(
            "open.invalid",
            path = path.display().to_string(),
            reason = error.to_string()
        ),
        ReadError::SourceMissing(path) | ReadError::SourceChanged(path) => {
            tr!("open.source", path = path.display().to_string())
        }
    }
}

/// The paths chosen in a system file dialog, `None` when it was cancelled
/// or could not open (then a notification says why).
pub(crate) fn prompt_result<T, E>(
    result: std::result::Result<Result<Option<T>>, E>,
    cx: &mut AsyncWindowContext,
) -> Option<T> {
    match result {
        Ok(Ok(Some(value))) => Some(value),
        // Cancelled.
        Ok(Ok(None)) | Err(_) => None,
        Ok(Err(error)) => {
            let reason = error.to_string();
            cx.update(|window, cx| {
                window.push_notification(
                    Notification::error(tr!("open.unavailable", reason = reason)),
                    cx,
                )
            })
            .ok();
            None
        }
    }
}
