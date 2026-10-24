//! The audit log of AI operations (`docs/design/ai-integration.md` §7.3):
//! records of MCP clients' calls wait here, per project, and are appended
//! to the project's `audit.jsonl` when it is saved, closed, or the
//! application quits; like the view state, they do not make the project
//! unsaved. "Export AI log…" writes a project's whole log to a file.

use std::path::Path;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;
use mw_project::ProjectId;
use mw_project_file::AuditRecord;

use crate::actions::ExportAiLog;
use crate::workbench::{Workbench, prompt_result};

impl Workbench {
    /// Keeps a client's record for the open project with the file id
    /// `project`.
    pub(crate) fn keep_audit(
        &mut self,
        project: mw_project_file::ProjectId,
        record: AuditRecord,
        cx: &App,
    ) {
        let found = self
            .catalog(cx)
            .projects()
            .iter()
            .find(|p| p.file_id() == project)
            .map(|p| p.id());
        if let Some(id) = found {
            self.audit.entry(id).or_default().push(record);
        }
    }

    /// Appends the records waiting for a saved project to its log. An
    /// untitled project keeps them until it is saved.
    pub(crate) fn write_audit(&mut self, project: ProjectId, cx: &App) {
        let Some(path) = self.catalog(cx).project(project).and_then(|p| p.path()) else {
            return;
        };
        let Some(records) = self.audit.get(&project).filter(|r| !r.is_empty()) else {
            return;
        };
        match mw_project_file::append_audit(path, records) {
            Ok(()) => {
                self.audit.remove(&project);
            }
            Err(error) => log::warn!(
                "could not append to the audit log of {}: {error}",
                path.display()
            ),
        }
    }

    /// The current project's log: what its file holds, then what waits.
    fn ai_log(&self, project: ProjectId, cx: &App) -> Vec<AuditRecord> {
        let saved = self
            .catalog(cx)
            .project(project)
            .and_then(|p| p.path())
            .map(|path| {
                mw_project_file::read_audit(path).unwrap_or_else(|error| {
                    log::warn!(
                        "could not read the audit log of {}: {error}",
                        path.display()
                    );
                    Vec::new()
                })
            })
            .unwrap_or_default();
        let waiting = self.audit.get(&project).cloned().unwrap_or_default();
        saved.into_iter().chain(waiting).collect()
    }

    pub(crate) fn export_ai_log(
        &mut self,
        _: &ExportAiLog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self
            .active_project
            .filter(|id| self.catalog(cx).project(*id).is_some())
        else {
            return;
        };
        let found = self.catalog(cx).project(project).expect("filtered");
        let name = found.name().to_string();
        let directory = found
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(dirs::document_dir)
            .unwrap_or_default();
        let records = self.ai_log(project, cx);
        if records.is_empty() {
            window.push_notification(Notification::info(tr!("ai_log.empty", name = name)), cx);
            return;
        }
        let suggested = format!("{name} AI.jsonl");
        let prompt = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |_, cx| {
            let Some(path) = prompt_result(prompt.await, cx) else {
                return;
            };
            let count = records.len();
            let target = path.clone();
            let written = cx
                .background_spawn(async move {
                    let text: String = records
                        .iter()
                        .map(|record| format!("{}\n", record.to_line()))
                        .collect();
                    std::fs::write(&target, text)
                })
                .await;
            let file = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            cx.update(|window, cx| {
                let notification = match written {
                    Ok(()) => {
                        Notification::success(tr!("ai_log.exported", count = count, file = file))
                    }
                    Err(error) => {
                        Notification::error(tr!("ai_log.failed", reason = error.to_string()))
                    }
                };
                window.push_notification(notification, cx);
            })
            .ok();
        })
        .detach();
    }
}
