//! Exporting the open structure (`docs/design/crystallography-model.md`
//! §9.2–9.4): the user picks a format in a dialog, then a file in the
//! system save panel. The text is written off the UI thread, and a
//! notification says where it went or why it could not be written.
//!
//! Formats that list atoms one by one (POSCAR, extended XYZ) cannot hold a
//! partially occupied site; the dialog disables them and says why.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::{ActiveTheme, Sizable, WindowExt, h_flex};
use gpui_kit::*;
use mw_crystal::Structure;
use mw_crystal_io::{ExportError, ImportedBlock, cif, poscar, xyz};

use crate::workspace::StructureWorkspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExportFormat {
    Cif,
    Poscar,
    Xyz,
}

impl ExportFormat {
    const ALL: [Self; 3] = [Self::Cif, Self::Poscar, Self::Xyz];

    fn label(self) -> SharedString {
        match self {
            Self::Cif => tr!("export.format.cif.label"),
            Self::Poscar => tr!("export.format.poscar.label"),
            Self::Xyz => tr!("export.format.xyz.label"),
        }
    }

    fn description(self) -> SharedString {
        match self {
            Self::Cif => tr!("export.format.cif.description"),
            Self::Poscar => tr!("export.format.poscar.description"),
            Self::Xyz => tr!("export.format.xyz.description"),
        }
    }

    /// Of the suggested file name. A POSCAR is often named just `POSCAR`;
    /// `.vasp` keeps several apart, and VESTA, ASE and pymatgen read it.
    fn extension(self) -> &'static str {
        match self {
            Self::Cif => "cif",
            Self::Poscar => "vasp",
            Self::Xyz => "xyz",
        }
    }

    /// Whether the structure can be written in this format.
    fn check(self, structure: &Structure) -> Result<(), ExportError> {
        match self {
            Self::Cif => Ok(()),
            Self::Poscar => poscar::check(structure),
            Self::Xyz => xyz::check(structure),
        }
    }
}

/// The format and folder of the last export in this session, offered
/// again next time.
#[derive(Default)]
struct LastExport {
    format: Option<ExportFormat>,
    directory: Option<PathBuf>,
}

impl Global for LastExport {}

/// Why a format cannot hold the structure, for the dialog.
fn reason(error: &ExportError) -> SharedString {
    let labels = |labels: &[String]| {
        const SHOWN: usize = 3;
        let separator = tr!("export.list_separator").to_string();
        let shown = labels[..labels.len().min(SHOWN)].join(&separator);
        if labels.len() > SHOWN {
            tr!(
                "export.more_labels",
                labels = shown,
                total = labels.len(),
                more = labels.len() - SHOWN
            )
            .to_string()
        } else {
            shown
        }
    };
    match error {
        ExportError::PartialOccupancy { labels: sites } => {
            tr!("export.unavailable.partial", labels = labels(sites))
        }
        ExportError::UnknownElement { labels: sites } => {
            tr!("export.unavailable.unknown_element", labels = labels(sites))
        }
        _ => tr!("export.unavailable.no_atoms"),
    }
}

/// A file name for the structure: its name without the characters file
/// systems reserve, and the format's extension.
fn file_name(name: &str, format: ExportFormat) -> String {
    let stem: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let stem = stem.trim();
    let stem = if stem.is_empty() { "structure" } else { stem };
    format!("{stem}.{}", format.extension())
}

/// What a confirmed export writes, taken from the document when the user
/// confirms.
struct ExportJob {
    format: ExportFormat,
    structure: Structure,
    source: Option<Arc<ImportedBlock>>,
    /// Whether the structure differs from the imported one: then the source's
    /// unrecognized fields are left out.
    changed: bool,
    /// The POSCAR comment line.
    comment: String,
}

#[derive(Debug)]
enum ExportFailure {
    Format(ExportError),
    Io(String),
}

impl ExportJob {
    /// The file's text, and for CIF the source fields left out because the
    /// structure changed. A CIF block is named after the source block, or
    /// else after the file.
    fn text(&self, file_stem: &str) -> Result<(String, Vec<String>), ExportError> {
        match self.format {
            ExportFormat::Cif => {
                let options = match &self.source {
                    Some(block) => {
                        cif::WriteOptions::new(&block.name).kept(&block.fields, self.changed)
                    }
                    None => cif::WriteOptions::new(file_stem),
                };
                let written = cif::write(&self.structure, &options);
                Ok((written.text, written.dropped))
            }
            ExportFormat::Poscar => {
                Ok((poscar::write(&self.structure, &self.comment)?, Vec::new()))
            }
            ExportFormat::Xyz => Ok((xyz::write(&self.structure)?, Vec::new())),
        }
    }

    /// Writes the file; blocks on I/O. Returns the CIF fields left out.
    fn write(&self, path: &Path) -> Result<Vec<String>, ExportFailure> {
        let stem = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let (text, dropped) = self.text(&stem).map_err(ExportFailure::Format)?;
        std::fs::write(path, text).map_err(|e| ExportFailure::Io(e.to_string()))?;
        Ok(dropped)
    }
}

impl StructureWorkspace {
    /// Opens the export dialog: the formats, those the structure does not
    /// fit disabled with the reason.
    pub(crate) fn open_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let structure = self.document.structure();
        let reasons: Vec<Option<SharedString>> = ExportFormat::ALL
            .iter()
            .map(|format| format.check(structure).err().map(|e| reason(&e)))
            .collect();
        let available = |format: &ExportFormat| {
            let index = ExportFormat::ALL.iter().position(|f| f == format);
            index.is_some_and(|i| reasons[i].is_none())
        };
        let initial = cx
            .try_global::<LastExport>()
            .and_then(|last| last.format)
            .filter(available)
            .unwrap_or(ExportFormat::Cif);
        let choice = cx.new(|_| initial);
        let title = tr!("export.title", name = self.document.name.to_string());
        let workspace = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let selected = *choice.read(cx);
            let radios = ExportFormat::ALL
                .iter()
                .zip(&reasons)
                .map(|(format, reason)| {
                    let detail = reason.clone().unwrap_or_else(|| format.description());
                    Radio::new(format.extension())
                        .w_full()
                        .label(format.label())
                        .disabled(reason.is_some())
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(detail),
                        )
                });
            let on_choose = {
                let choice = choice.clone();
                move |index: &usize, window: &mut Window, cx: &mut App| {
                    choice.update(cx, |choice, _| *choice = ExportFormat::ALL[*index]);
                    window.refresh();
                }
            };
            // The button and Enter both export.
            let confirm = {
                let (choice, workspace) = (choice.clone(), workspace.clone());
                Rc::new(move |window: &mut Window, cx: &mut App| {
                    let format = *choice.read(cx);
                    workspace
                        .update(cx, |this, cx| this.save_export(format, window, cx))
                        .ok();
                })
            };
            dialog
                .title(title.clone())
                .child(
                    RadioGroup::vertical("export-format")
                        .w_full()
                        .children(radios)
                        .selected_index(ExportFormat::ALL.iter().position(|f| *f == selected))
                        .on_click(on_choose),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("export-cancel")
                                .label(tr!("dialog.cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("export-save")
                                .primary()
                                .label(tr!("export.save"))
                                .on_click({
                                    let confirm = confirm.clone();
                                    move |_, window, cx| {
                                        window.close_dialog(cx);
                                        confirm(window, cx);
                                    }
                                }),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    confirm(window, cx);
                    true
                })
        });
    }

    /// Asks where to save, then writes the file in the background.
    fn save_export(&mut self, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        let directory = cx
            .try_global::<LastExport>()
            .and_then(|last| last.directory.clone())
            .or_else(std::env::home_dir)
            .unwrap_or_default();
        let suggested = file_name(&self.document.name, format);
        let path = cx.prompt_for_new_path(&directory, Some(&suggested));
        let job = ExportJob {
            format,
            structure: self.document.structure().clone(),
            source: self.document.source.clone(),
            changed: self.document.changed_since_import(),
            comment: format!("{} · {}", self.document.name, self.document.project_name),
        };
        cx.spawn_in(window, async move |_, cx| {
            let path = match path.await {
                Ok(Ok(Some(path))) => path,
                // Cancelled.
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    let reason = error.to_string();
                    cx.update(|window, cx| {
                        window.push_notification(
                            Notification::error(tr!("export.panel_unavailable", reason = reason)),
                            cx,
                        )
                    })
                    .ok();
                    return;
                }
            };
            let written = cx
                .background_spawn({
                    let path = path.clone();
                    async move { job.write(&path) }
                })
                .await;
            cx.update(|window, cx| {
                cx.set_global(LastExport {
                    format: Some(format),
                    directory: path.parent().map(Path::to_path_buf),
                });
                report(&path, written, window, cx);
            })
            .ok();
        })
        .detach();
    }
}

/// Tells the user where the file went, or why it was not written.
fn report(
    path: &Path,
    written: Result<Vec<String>, ExportFailure>,
    window: &mut Window,
    cx: &mut App,
) {
    let file = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let notification = match written {
        Ok(dropped) if dropped.is_empty() => Notification::success(tr!("export.done", file = file)),
        Ok(dropped) => Notification::warning(tr!(
            "export.done_dropped",
            file = file,
            count = dropped.len(),
            tags = dropped.join(", ")
        )),
        Err(failure) => {
            let reason = match failure {
                ExportFailure::Format(error) => reason(&error).to_string(),
                ExportFailure::Io(reason) => reason,
            };
            window.push_notification(
                Notification::error(tr!("export.failed", file = file, reason = reason)),
                cx,
            );
            return;
        }
    };
    let path = path.to_path_buf();
    let label = if cfg!(target_os = "macos") {
        tr!("export.reveal_in_finder")
    } else {
        tr!("export.reveal_in_folder")
    };
    let notification = notification
        .action(move |_, _, _| {
            let path = path.clone();
            Button::new("export-reveal")
                .outline()
                .small()
                .label(label.clone())
                .on_click(move |_, _, cx| cx.reveal_path(&path))
        })
        .autohide(true);
    window.push_notification(notification, cx);
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob would shadow the `test` attribute.
    use std::sync::Arc;

    use mw_crystal::Structure;
    use mw_crystal::sample::{self, SampleKey};
    use mw_crystal_io::{ExportError, ImportedBlock, cif};

    use super::{ExportFormat, ExportJob, file_name, reason};

    fn job(
        format: ExportFormat,
        structure: Structure,
        source: Option<Arc<ImportedBlock>>,
    ) -> ExportJob {
        let changed = source
            .as_ref()
            .is_none_or(|block| block.structure.as_ref() != Some(&structure));
        ExportJob {
            format,
            structure,
            source,
            changed,
            comment: "金红石 · TiO₂".into(),
        }
    }

    #[test]
    fn a_cif_block_is_named_after_its_source_or_the_file() {
        let rutile = sample::load(SampleKey::Rutile).structure;
        let (text, dropped) = job(ExportFormat::Cif, rutile, None).text("TiO2").unwrap();
        assert!(text.contains("\ndata_TiO2\n"));
        assert!(dropped.is_empty());

        let report = cif::read_str(
            "data_halite\n_space_group_IT_number 225\n_journal_name_full 'Some journal'\n\
             _cell_length_a 5.64\n_cell_length_b 5.64\n_cell_length_c 5.64\n\
             loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
             Na1 0 0 0\nCl1 0.5 0.5 0.5\n",
        );
        let block = Arc::new(report.blocks[0].clone());
        let structure = block.structure.clone().unwrap();
        let (text, _) = job(ExportFormat::Cif, structure, Some(block))
            .text("anything")
            .unwrap();
        assert!(text.contains("\ndata_halite\n"));
        assert!(text.contains("_journal_name_full"));
    }

    #[test]
    fn poscar_carries_the_comment() {
        let rutile = sample::load(SampleKey::Rutile).structure;
        let (text, _) = job(ExportFormat::Poscar, rutile, None).text("x").unwrap();
        assert!(text.starts_with("金红石 · TiO₂\n1.0\n"));
    }

    #[test]
    fn atom_lists_are_unavailable_for_partial_occupancy() {
        let report = cif::read_str(
            "data_x\n_space_group_IT_number 221\n\
             _cell_length_a 4\n_cell_length_b 4\n_cell_length_c 4\n\
             loop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\n\
             _atom_site_occupancy\nBa1 0 0 0 0.5\nSr1 0 0 0 0.5\nTi1 0.5 0.5 0.5 1\n",
        );
        let structure = report.blocks[0].structure.clone().unwrap();
        assert!(ExportFormat::Cif.check(&structure).is_ok());
        let error = ExportFormat::Poscar.check(&structure).unwrap_err();
        assert_eq!(
            reason(&error).to_string(),
            tr!("export.unavailable.partial", labels = "Ba1").to_string()
        );
    }

    #[test]
    fn long_label_lists_are_shortened() {
        let labels = ["A1", "A2", "A3", "A4", "A5"].map(String::from).to_vec();
        let text = reason(&ExportError::UnknownElement { labels }).to_string();
        assert!(text.contains("A3"), "{text}");
        assert!(!text.contains("A4"), "{text}");
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name("金红石", ExportFormat::Cif), "金红石.cif");
        assert_eq!(file_name("a/b: c", ExportFormat::Poscar), "a-b- c.vasp");
        assert_eq!(file_name(" ", ExportFormat::Xyz), "structure.xyz");
    }
}
