//! Opening structure files (`docs/design/ui-shell.md` §5): reading them off
//! the UI thread, naming the structures they hold, and telling the user what
//! happened — nothing when a file imports cleanly, since the opened
//! structure shows it; a notification when it has diagnostics or failed;
//! the error list, with source lines, in a dialog.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme, Sizable, StyledExt, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal_io::{Diagnostic, ImportReport, ImportedBlock, Severity, cif};

use crate::diagnostics::DiagnosticRow;

/// Why a file yielded no report at all.
#[derive(Clone, Debug)]
enum ReadError {
    /// Not a format the importer reads; only CIF so far.
    Unsupported,
    Io(String),
}

/// One file the user asked to open, read and parsed.
#[derive(Clone, Debug)]
pub struct OpenedFile {
    path: PathBuf,
    /// The file's bytes, for the project's copy.
    bytes: Option<Arc<[u8]>>,
    result: Result<ImportReport, ReadError>,
}

impl OpenedFile {
    /// Reads and parses the file. Blocks on file I/O; run it in a background
    /// task.
    pub fn read(path: PathBuf) -> Self {
        let supported = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cif"));
        let read = if supported {
            std::fs::read(&path).map_err(|e| ReadError::Io(e.to_string()))
        } else {
            Err(ReadError::Unsupported)
        };
        let (bytes, result) = match read {
            Ok(bytes) => {
                let bytes: Arc<[u8]> = bytes.into();
                let report = cif::read(&bytes);
                (Some(bytes), Ok(report))
            }
            Err(error) => (None, Err(error)),
        };
        Self {
            path,
            bytes,
            result,
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The file's bytes, if it could be read.
    pub fn bytes(&self) -> Option<Arc<[u8]>> {
        self.bytes.clone()
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }

    /// The structures in the file, each named for the project: the file name
    /// without extension, plus the data block name when there are several.
    /// Problems that belong to the whole file are added to each structure's
    /// own diagnostics.
    pub fn structures(&self) -> Vec<(String, Arc<ImportedBlock>)> {
        let Ok(report) = &self.result else {
            return Vec::new();
        };
        let stem = self
            .path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let several = report.structures().count() > 1;
        report
            .structures()
            .map(|(block, _)| {
                let name = if several {
                    format!("{stem} · {}", block.name)
                } else {
                    stem.clone()
                };
                let mut block = block.clone();
                block
                    .diagnostics
                    .splice(0..0, report.diagnostics.iter().cloned());
                (name, Arc::new(block))
            })
            .collect()
    }

    /// Tells the user how the import went, unless it went cleanly.
    pub fn notify(&self, window: &mut Window, cx: &mut App) {
        let file = self.file_name();
        let report = match &self.result {
            Ok(report) => report,
            Err(ReadError::Unsupported) => {
                window.push_notification(
                    Notification::error(tr!("import.unsupported", file = file)),
                    cx,
                );
                return;
            }
            Err(ReadError::Io(reason)) => {
                window.push_notification(
                    Notification::error(tr!("import.unreadable", file = file, reason = reason)),
                    cx,
                );
                return;
            }
        };

        let imported = report.structures().count();
        let failed: Vec<&ImportedBlock> = report.failed_blocks().collect();
        let file_errors = report
            .diagnostics
            .iter()
            .filter(|d| d.severity() == Severity::Error)
            .count();
        let errors = file_errors
            + failed
                .iter()
                .flat_map(|b| &b.diagnostics)
                .filter(|d| d.severity() == Severity::Error)
                .count();

        let notification = if imported == 0 && errors > 0 {
            Notification::error(tr!("import.failed", file = file.clone(), count = errors))
        } else if imported == 0 {
            Notification::warning(tr!("import.no_structure", file = file.clone()))
        } else if !failed.is_empty() {
            Notification::warning(tr!(
                "import.partly_failed",
                file = file.clone(),
                imported = imported,
                failed = failed.len()
            ))
        } else {
            let diagnostics: Vec<&Diagnostic> = report
                .diagnostics
                .iter()
                .chain(report.structures().flat_map(|(b, _)| &b.diagnostics))
                .collect();
            if diagnostics.is_empty() {
                return;
            }
            let message = tr!(
                "import.with_diagnostics",
                file = file.clone(),
                count = diagnostics.len()
            );
            if diagnostics
                .iter()
                .any(|d| d.severity() == Severity::Warning)
            {
                Notification::warning(message)
            } else {
                Notification::info(message)
            }
        };

        // Only failures have details outside the opened structure's
        // diagnostics tab.
        let notification = if errors > 0 {
            let this = self.clone();
            notification.action(move |_, _, _| {
                let this = this.clone();
                Button::new("import-show-errors")
                    .outline()
                    .small()
                    .label(tr!("import.show_errors"))
                    .on_click(move |_, window, cx| this.show_errors(window, cx))
            })
        } else {
            notification
        };
        window.push_notification(notification, cx);
    }

    /// Opens the dialog that lists why the file, or some of its data blocks,
    /// could not be imported. Does nothing for a file that was not read.
    pub fn show_errors(&self, window: &mut Window, cx: &mut App) {
        if let Ok(report) = &self.result {
            open_errors(self.file_name(), report.clone(), window, cx);
        }
    }
}

fn open_errors(file: String, report: ImportReport, window: &mut Window, cx: &mut App) {
    let several_blocks = report.blocks.len() > 1;
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        let file_rows = report.diagnostics.iter().map(DiagnosticRow::from_import);
        let blocks = report.failed_blocks().map(|block| {
            v_flex()
                .gap_1()
                .when(several_blocks, |this| {
                    this.child(
                        div()
                            .px_2()
                            .pt_2()
                            .text_xs()
                            .font_semibold()
                            .text_color(theme.muted_foreground)
                            .child(tr!("import.errors.block", name = block.name.clone())),
                    )
                })
                .children(block.diagnostics.iter().map(DiagnosticRow::from_import))
        });
        dialog
            .title(tr!("import.errors.title", file = file.clone()))
            .w_128()
            .child(
                div()
                    .id("import-errors")
                    .max_h_96()
                    .overflow_y_scroll()
                    .child(v_flex().gap_1().children(file_rows).children(blocks)),
            )
            .footer(
                h_flex().w_full().justify_end().child(
                    Button::new("import-errors-close")
                        .label(tr!("import.errors.close"))
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
    });
}
