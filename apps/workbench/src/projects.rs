//! Opening, saving and closing projects (`docs/design/project-format.md`
//! §9–§11, `docs/design/ui-shell.md` §3): reading them in the background,
//! the save panel for untitled projects and "save as", asking before
//! unsaved changes are lost, and what survives a restart.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{WindowExt, h_flex};
use gpui_kit::*;
use mw_crystal::Checkpoint;
use mw_project::{ProjectId, SaveSnapshot, StructureEntryId};
use mw_project_file::{EXTENSION, Manifest, ProjectDir, ReadError, SaveError, ViewState};
use mw_structure::StructureWorkspace;

use crate::actions::*;
use crate::config::AppPaths;
use crate::workbench::{Workbench, prompt_result, read_error};

/// What reading one project gave.
type ReadProject = (
    PathBuf,
    Result<(ProjectDir, Manifest, ViewState), ReadError>,
);

/// Reads a project's manifest and view state. Blocks on file I/O.
fn read_project(path: PathBuf) -> ReadProject {
    let result = mw_project_file::read_manifest(&path).map(|(dir, manifest)| {
        let view = dir.read_view().unwrap_or_default();
        (dir, manifest, view)
    });
    (path, result)
}

/// The documents a save writes, and the states to mark saved once it is
/// written.
type Prepared = (
    mw_project_file::Save,
    SaveSnapshot,
    Vec<(Entity<StructureWorkspace>, Checkpoint)>,
);

/// A name usable as a file name.
fn file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) { '-' } else { c })
        .collect();
    let stem = stem.trim();
    if stem.is_empty() {
        "project".into()
    } else {
        stem.into()
    }
}

/// The path with the project extension, which the save panel may leave out.
fn with_extension(path: PathBuf) -> PathBuf {
    if path.extension().is_some_and(|e| e == EXTENSION) {
        return path;
    }
    let mut name = path.into_os_string();
    name.push(".");
    name.push(EXTENSION);
    PathBuf::from(name)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

impl Workbench {
    /// Reads projects in the background, in order, adds them to the sidebar
    /// and shows `activate`'s last structure (or the first project's).
    /// Projects already open are shown, not opened twice.
    pub(crate) fn open_projects(
        &mut self,
        paths: Vec<PathBuf>,
        activate: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let read =
            cx.background_spawn(
                async move { paths.into_iter().map(read_project).collect::<Vec<_>>() },
            );
        cx.spawn_in(window, async move |this, cx| {
            let projects = read.await;
            this.update_in(cx, |this, window, cx| {
                this.add_projects(projects, activate, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn add_projects(
        &mut self,
        projects: Vec<ReadProject>,
        activate: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut shown = None;
        for (path, result) in projects {
            match result {
                Ok((dir, manifest, view)) => {
                    let (id, name, file_id) = self.sidebar.update(cx, |sidebar, cx| {
                        sidebar.update_catalog(cx, |catalog| {
                            let id = catalog
                                .find(&path, manifest.id)
                                .unwrap_or_else(|| catalog.open(dir, manifest, view));
                            let project = catalog.project(id).expect("just added");
                            (id, project.name().to_string(), project.file_id())
                        })
                    });
                    self.session.remember(&path, file_id, &name);
                    if shown.is_none() || activate.as_deref() == Some(path.as_path()) {
                        shown = Some(id);
                    }
                }
                Err(error) => {
                    if matches!(error, ReadError::NotAProject(_)) || !path.exists() {
                        self.session.forget(&path);
                    }
                    window.push_notification(
                        Notification::error(tr!(
                            "open.project_failed",
                            reason = read_error(&error).to_string()
                        )),
                        cx,
                    );
                }
            }
        }
        self.session.save(cx.global::<AppPaths>());
        if let Some(project) = shown {
            self.show_project(project, window, cx);
        }
        self.refresh_status(cx);
        cx.notify();
    }

    /// Shows the structure last open in a project, or its first.
    fn show_project(&mut self, project: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        self.active_project = Some(project);
        let Some(found) = self.catalog(cx).project(project) else {
            return;
        };
        let structure = self
            .last_shown
            .get(&project)
            .copied()
            .or_else(|| {
                let open = found.view().open?;
                found
                    .structures()
                    .iter()
                    .find(|s| s.entry().id == open)
                    .map(|s| s.id())
            })
            .or_else(|| found.structures().first().map(|s| s.id()));
        match structure {
            Some(id) => self.open_structure(id, window, cx),
            None => {
                self.current = None;
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.set_selected(None, cx));
                self.focus_handle.focus(window, cx);
                cx.notify();
            }
        }
    }

    /// Installs the sample projects, unless they are there already, and
    /// opens them.
    pub(crate) fn open_samples(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = cx.global::<AppPaths>().samples_dir();
        let install = cx.background_spawn(async move {
            std::fs::create_dir_all(&dir)?;
            mw_project_file::samples::install(&dir)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = install.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(paths) => {
                    let first = paths.first().cloned();
                    this.open_projects(paths, first, window, cx);
                }
                Err(error) => window.push_notification(
                    Notification::error(tr!("open.samples_failed", reason = error.to_string())),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// Adds an empty, untitled project and returns it.
    pub(crate) fn create_project(&mut self, cx: &mut Context<Self>) -> ProjectId {
        let base = tr!("project.untitled").to_string();
        let names: Vec<String> = self
            .catalog(cx)
            .projects()
            .iter()
            .map(|p| p.name().to_string())
            .collect();
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base} {n}")
                }
            })
            .find(|name| !names.contains(name))
            .expect("some name is free");
        let id = self.sidebar.update(cx, |sidebar, cx| {
            sidebar.update_catalog(cx, |catalog| catalog.create(name))
        });
        self.active_project = Some(id);
        id
    }

    /// The project new files go to and ⌘S saves.
    fn active_project(&self, cx: &App) -> Option<ProjectId> {
        self.active_project
            .filter(|id| self.catalog(cx).project(*id).is_some())
    }

    /// The view state to save for a project: the open structures as shown
    /// now, the others as last read.
    pub(crate) fn view_state(&self, project: ProjectId, cx: &App) -> ViewState {
        let Some(project) = self.catalog(cx).project(project) else {
            return ViewState::default();
        };
        let mut view = project.view().clone();
        for opened in &self.opened {
            if let Some(structure) = project.structure(opened.id) {
                view.entries
                    .insert(structure.entry().id, opened.workspace.read(cx).view(cx));
            }
        }
        view.entries
            .retain(|id, _| project.structures().iter().any(|s| s.entry().id == *id));
        if let Some(shown) = self
            .last_shown
            .get(&project.id())
            .and_then(|id| project.structure(*id))
        {
            view.open = Some(shown.entry().id);
        }
        view
    }

    // MARK: Saving

    /// Saves a project: to its directory, or to one the user picks for an
    /// untitled project and for "save as". Resolves to whether it was saved.
    pub(crate) fn save_project(
        &mut self,
        project: ProjectId,
        save_as: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(found) = self.catalog(cx).project(project) else {
            return Task::ready(false);
        };
        let current = found.path().map(Path::to_path_buf);
        let ask = save_as || current.is_none();
        let prompt = ask.then(|| {
            let directory = current
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .or_else(dirs::document_dir)
                .or_else(std::env::home_dir)
                .unwrap_or_default();
            let suggested = format!("{}.{EXTENSION}", file_stem(found.name()));
            cx.prompt_for_new_path(&directory, Some(&suggested))
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match prompt {
                Some(prompt) => match prompt_result(prompt.await, cx) {
                    Some(path) => with_extension(path),
                    None => return false,
                },
                None => current.expect("a saved project has a directory"),
            };
            let prepared = this.update_in(cx, |this, window, cx| {
                this.prepare_save(project, &path, ask, window, cx)
            });
            let Ok(Some((save, snapshot, checkpoints))) = prepared else {
                return false;
            };
            let target = path.clone();
            let result = cx
                .background_spawn(async move { mw_project_file::save(&target, &save) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.finish_save(project, &path, result, snapshot, checkpoints, window, cx)
            })
            .unwrap_or(false)
        })
    }

    /// Collects what to write. Refuses a new location that is taken:
    /// another project or folder there is never overwritten.
    fn prepare_save(
        &mut self,
        project: ProjectId,
        path: &Path,
        renamed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Prepared> {
        let found = self.catalog(cx).project(project)?;
        let own = found.path().is_some_and(|own| same_dir(own, path));
        if path.exists() && !own {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            window.push_notification(Notification::error(tr!("save.exists", name = name)), cx);
            return None;
        }
        let mut structures = Vec::new();
        let mut checkpoints = Vec::new();
        for opened in &self.opened {
            let Some(entry) = found.structure(opened.id) else {
                continue;
            };
            let workspace = opened.workspace.read(cx);
            if workspace.is_modified() || entry.pending().is_some() {
                let (file, checkpoint) = workspace.file_to_save();
                structures.push((entry.entry().id, file));
                checkpoints.push((opened.workspace.clone(), checkpoint));
            }
        }
        let view = self.view_state(project, cx);
        let mut save = found.save_request(structures, view);
        // Saved under a new name, the project takes it (design §5).
        if renamed && let Some(stem) = path.file_stem() {
            save.manifest.name = stem.to_string_lossy().into_owned();
        }
        let snapshot = self.catalog(cx).snapshot(project, &save)?;
        Some((save, snapshot, checkpoints))
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_save(
        &mut self,
        project: ProjectId,
        path: &Path,
        result: Result<ProjectDir, SaveError>,
        snapshot: SaveSnapshot,
        checkpoints: Vec<(Entity<StructureWorkspace>, Checkpoint)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match result {
            Ok(dir) => {
                let saved = self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.update_catalog(cx, |catalog| {
                        catalog.saved(project, dir, &snapshot);
                        let found = catalog.project(project)?;
                        Some((found.file_id(), found.name().to_string()))
                    })
                });
                for (workspace, checkpoint) in checkpoints {
                    workspace.update(cx, |workspace, cx| workspace.mark_saved(checkpoint, cx));
                }
                if let Some((file_id, name)) = saved {
                    self.session.remember(path, file_id, &name);
                    self.session.save(cx.global::<AppPaths>());
                }
                self.refresh_status(cx);
                true
            }
            Err(error) => {
                window.push_notification(
                    Notification::error(tr!("save.failed", reason = error.to_string())),
                    cx,
                );
                false
            }
        }
    }

    // MARK: Closing

    /// Closes a project, asking first when it has unsaved changes.
    pub(crate) fn close_project(
        &mut self,
        project: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_project_modified(project, cx) {
            self.remove_project(project, window, cx);
            return;
        }
        let Some(name) = self
            .catalog(cx)
            .project(project)
            .map(|p| p.name().to_string())
        else {
            return;
        };
        let this = cx.entity().downgrade();
        let discard = this.clone();
        self.confirm_unsaved(
            tr!("unsaved.close_title", name = name),
            None,
            move |window, cx| {
                let this = this.clone();
                window.defer(cx, move |window, cx| {
                    this.update(cx, |workbench, cx| {
                        let save = workbench.save_project(project, false, window, cx);
                        cx.spawn_in(window, async move |this, cx| {
                            if save.await {
                                this.update_in(cx, |this, window, cx| {
                                    this.remove_project(project, window, cx)
                                })
                                .ok();
                            }
                        })
                        .detach();
                    })
                    .ok();
                });
            },
            move |window, cx| {
                discard
                    .update(cx, |this, cx| this.remove_project(project, window, cx))
                    .ok();
            },
            window,
            cx,
        );
    }

    /// Closes a project without asking: its open structures go, and its
    /// view state is written.
    fn remove_project(&mut self, project: ProjectId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.catalog(cx).project(project).and_then(|p| p.path())
            && let Err(error) = mw_project_file::write_view(path, &self.view_state(project, cx))
        {
            log::warn!(
                "could not write the view state of {}: {error}",
                path.display()
            );
        }
        let Some(closed) = self.sidebar.update(cx, |sidebar, cx| {
            sidebar.update_catalog(cx, |catalog| catalog.close(project))
        }) else {
            return;
        };
        let ids: Vec<StructureEntryId> = closed.structures().iter().map(|s| s.id()).collect();
        self.opened.retain(|opened| !ids.contains(&opened.id));
        self.last_shown.remove(&project);
        if self.active_project == Some(project) {
            self.active_project = None;
        }
        if self.current.is_some_and(|id| ids.contains(&id)) {
            self.current = None;
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_selected(None, cx));
            self.focus_handle.focus(window, cx);
        }
        self.refresh_status(cx);
        cx.notify();
    }

    /// Asks before quitting when projects have unsaved changes. Returns
    /// whether the application may quit now.
    pub(crate) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let modified: Vec<(ProjectId, String)> = self
            .catalog(cx)
            .projects()
            .iter()
            .map(|p| (p.id(), p.name().to_string()))
            .filter(|(id, _)| self.is_project_modified(*id, cx))
            .collect();
        if modified.is_empty() {
            return true;
        }
        let (title, description) = match &modified[..] {
            [(_, name)] => (tr!("unsaved.quit_title", name = name.clone()), None),
            _ => {
                let names: Vec<&str> = modified.iter().map(|(_, n)| n.as_str()).collect();
                (
                    tr!("unsaved.quit_title_many", count = modified.len()),
                    Some(tr!(
                        "unsaved.quit_projects",
                        names = names.join(&tr!("unsaved.separator"))
                    )),
                )
            }
        };
        let projects: Vec<ProjectId> = modified.into_iter().map(|(id, _)| id).collect();
        let this = cx.entity().downgrade();
        self.confirm_unsaved(
            title,
            description,
            move |window, cx| {
                let this = this.clone();
                let projects = projects.clone();
                window.defer(cx, move |window, cx| {
                    this.update(cx, |_, cx| {
                        cx.spawn_in(window, async move |this, cx| {
                            for project in projects {
                                let Ok(save) = this.update_in(cx, |this, window, cx| {
                                    this.save_project(project, false, window, cx)
                                }) else {
                                    return;
                                };
                                if !save.await {
                                    return;
                                }
                            }
                            cx.update(|_, cx| cx.quit()).ok();
                        })
                        .detach();
                    })
                    .ok();
                });
            },
            |_, cx| cx.quit(),
            window,
            cx,
        );
        false
    }

    /// The question before unsaved changes would be lost: `Cancel`,
    /// `Don't save`, and `Save`, the default.
    fn confirm_unsaved(
        &self,
        title: SharedString,
        description: Option<SharedString>,
        save: impl Fn(&mut Window, &mut App) + 'static,
        discard: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let save = std::rc::Rc::new(save);
        let discard = std::rc::Rc::new(discard);
        let body = match description {
            Some(projects) => tr!("unsaved.description_with", projects = projects.to_string()),
            None => tr!("unsaved.description"),
        };
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let save = save.clone();
            let discard = discard.clone();
            dialog
                .title(title.clone())
                .description(body.clone())
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("unsaved-cancel")
                                .label(tr!("unsaved.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("unsaved-discard")
                                .label(tr!("unsaved.discard"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    discard(window, cx);
                                }),
                        )
                        .child(
                            Button::new("unsaved-save")
                                .primary()
                                .label(tr!("unsaved.save"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    save(window, cx);
                                }),
                        ),
                )
        });
    }

    /// Writes what a restart restores: each saved project's view state, and
    /// the projects open now.
    pub(crate) fn persist(&mut self, cx: &mut Context<Self>) {
        let mut open = Vec::new();
        for project in self.catalog(cx).projects() {
            let Some(path) = project.path() else {
                continue;
            };
            if let Err(error) =
                mw_project_file::write_view(path, &self.view_state(project.id(), cx))
            {
                log::warn!(
                    "could not write the view state of {}: {error}",
                    path.display()
                );
            }
            open.push(path.to_path_buf());
        }
        self.session.open = open;
        self.session.active = self
            .active_project(cx)
            .and_then(|id| self.catalog(cx).project(id))
            .and_then(|p| p.path())
            .map(Path::to_path_buf);
        self.session.save(cx.global::<AppPaths>());
    }

    // MARK: Actions

    pub(crate) fn new_project(
        &mut self,
        _: &NewProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_project(cx);
        self.current = None;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_selected(None, cx));
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_project(
        &mut self,
        _: &OpenProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(paths) = prompt_result(paths.await, cx) else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let activate = paths.first().cloned();
                this.open_projects(paths, activate, window, cx)
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn save_active_project(
        &mut self,
        _: &SaveProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.active_project(cx) {
            self.save_project(project, false, window, cx).detach();
        }
    }

    pub(crate) fn save_active_project_as(
        &mut self,
        _: &SaveProjectAs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.active_project(cx) {
            self.save_project(project, true, window, cx).detach();
        }
    }

    pub(crate) fn close_active_project(
        &mut self,
        _: &CloseProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.active_project(cx) {
            self.close_project(project, window, cx);
        }
    }

    pub(crate) fn open_sample_projects(
        &mut self,
        _: &OpenSampleProjects,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_samples(window, cx);
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob would shadow the `test` attribute.
    use std::path::PathBuf;

    use super::{file_stem, with_extension};

    #[test]
    fn project_paths() {
        assert_eq!(with_extension("/a/b".into()), PathBuf::from("/a/b.mwproj"));
        assert_eq!(
            with_extension("/a/b.v2".into()),
            PathBuf::from("/a/b.v2.mwproj")
        );
        assert_eq!(
            with_extension("/a/b.mwproj".into()),
            PathBuf::from("/a/b.mwproj")
        );
        assert_eq!(file_stem("TiO₂ 相研究"), "TiO₂ 相研究");
        assert_eq!(file_stem("a/b: c"), "a-b- c");
        assert_eq!(file_stem("  "), "project");
    }
}
