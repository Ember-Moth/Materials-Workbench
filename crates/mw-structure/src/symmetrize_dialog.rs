//! The "Symmetrize…" dialog (crystallography design §7.10): searches the
//! symmetry the atoms have in the background, on a snapshot of the
//! structure, and previews the result: space group, sites, atoms in the
//! cell, the largest shift of an atom and what happens to the cell. The
//! tolerance and standardizing can change; each change searches again.
//! Confirming runs one `Symmetrize`, which undo reverts.

use std::sync::Arc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Sizable, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal::{DetectSymmetry, Edit, Structure, Symmetrization};

use crate::workspace::StructureWorkspace;
use crate::{cell_dialog, editing, format, panel};

/// What the search with the current values found.
enum Preview {
    Searching,
    Found(Box<Symmetrization>),
    /// Why the structure cannot be symmetrized with these values.
    Refused(SharedString),
}

/// The dialog's state: the structure it was opened on, the tolerance field,
/// whether to standardize, and the preview of the search running or done.
pub(crate) struct SymmetrizeDialog {
    structure: Arc<Structure>,
    atoms: usize,
    pub(crate) tolerance: Entity<InputState>,
    standardize: bool,
    preview: Preview,
    /// The last search started; replacing it drops the older one, whose
    /// result would no longer match the fields.
    search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SymmetrizeDialog {
    fn new(
        structure: Arc<Structure>,
        atoms: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let symprec = format::tolerance(structure.tolerances.symprec);
        let tolerance = cx.new(|cx| InputState::new(window, cx).default_value(symprec));
        let subscriptions =
            vec![
                cx.subscribe_in(&tolerance, window, |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.search(cx);
                    }
                }),
            ];
        let mut dialog = Self {
            structure,
            atoms,
            tolerance,
            standardize: false,
            preview: Preview::Searching,
            search: None,
            _subscriptions: subscriptions,
        };
        dialog.search(cx);
        dialog
    }

    /// The search the fields ask for, or why they ask for none.
    fn detect(&self, cx: &App) -> Result<DetectSymmetry, SharedString> {
        let text = self.tolerance.read(cx).value();
        let symprec = text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0)
            .ok_or_else(|| tr!("dialog.symmetrize.not_a_tolerance"))?;
        Ok(DetectSymmetry {
            symprec,
            standardize: self.standardize,
        })
    }

    /// The edit that confirming runs: the structure's own tolerance is
    /// left out, so the history records only a tolerance the user chose.
    pub(crate) fn edit(&self, cx: &App) -> Result<Edit, SharedString> {
        let detect = self.detect(cx)?;
        let own = (detect.symprec - self.structure.tolerances.symprec).abs() < 1e-12;
        Ok(Edit::Symmetrize {
            symprec: (!own).then_some(detect.symprec),
            standardize: detect.standardize,
        })
    }

    pub(crate) fn set_standardize(&mut self, standardize: bool, cx: &mut Context<Self>) {
        self.standardize = standardize;
        self.search(cx);
    }

    /// Starts searching with the current values, in the background.
    fn search(&mut self, cx: &mut Context<Self>) {
        match self.detect(cx) {
            Err(reason) => {
                self.search = None;
                self.preview = Preview::Refused(reason);
            }
            Ok(detect) => {
                self.preview = Preview::Searching;
                let structure = self.structure.clone();
                let task = cx.background_spawn(async move { detect.run(&structure) });
                self.search = Some(cx.spawn(async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, cx| {
                        this.preview = match result {
                            Ok(found) => Preview::Found(Box::new(found)),
                            Err(error) => Preview::Refused(editing::symmetrize_error_text(
                                &error,
                                &this.structure,
                            )),
                        };
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
        cx.notify();
    }

    /// What the search found, once it is done.
    #[cfg(test)]
    pub(crate) fn found(&self) -> Option<&Symmetrization> {
        match &self.preview {
            Preview::Found(found) => Some(found),
            _ => None,
        }
    }

    /// Why the values cannot be applied, once the search is done.
    #[cfg(test)]
    pub(crate) fn refusal(&self) -> Option<SharedString> {
        match &self.preview {
            Preview::Refused(reason) => Some(reason.clone()),
            _ => None,
        }
    }

    fn render_found(&self, found: &Symmetrization, cx: &App) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        // A label beside the first line of its value, as the cell dialog.
        let row = |label: SharedString, value: AnyElement| {
            h_flex()
                .items_start()
                .gap_3()
                .child(div().w_20().flex_none().text_color(muted).child(label))
                .child(div().flex_1().min_w_0().child(value))
        };
        let note = |text: SharedString| div().text_xs().text_color(muted).child(text);
        let structure = &found.structure;
        let atoms = mw_crystal::orbits(structure)
            .iter()
            .map(|orbit| orbit.positions.len())
            .sum::<usize>();
        let label = |site| {
            structure
                .site(site)
                .map_or_else(String::new, |s| s.label().to_string())
        };
        let shift = match found.moved_most {
            Some(site) if found.max_displacement > 0.0 => tr!(
                "dialog.symmetrize.displacement_value",
                distance = format::shift(found.max_displacement),
                label = label(site)
            ),
            _ => tr!("dialog.symmetrize.displacement_none"),
        };
        let params = structure.lattice.params();
        let cell = if found.cell_changed || params != self.structure.lattice.params() {
            v_flex()
                .children(cell_dialog::cell_lines(params))
                .into_any_element()
        } else {
            tr!("dialog.symmetrize.cell_unchanged").into_any_element()
        };
        let subgroup = found
            .space_group
            .as_ref()
            .filter(|sg| sg.number != found.detected_number)
            .map(|_| {
                tr!(
                    "dialog.symmetrize.subgroup",
                    space_group =
                        panel::space_group_name(found.detected_number, found.detected_symbol)
                            .to_string()
                )
            });
        v_flex()
            .gap_1()
            .child(row(
                tr!("panel.symmetry.space_group"),
                cell_dialog::setting_text(structure).into_any_element(),
            ))
            .child(row(
                tr!("dialog.symmetrize.sites"),
                tr!(
                    "dialog.symmetrize.sites_value",
                    count = structure.sites.len()
                )
                .into_any_element(),
            ))
            .child(row(
                tr!("dialog.cell.atoms"),
                tr!(
                    "dialog.cell.atoms_value",
                    before = self.atoms,
                    after = atoms
                )
                .into_any_element(),
            ))
            .child(row(
                tr!("dialog.symmetrize.displacement"),
                shift.into_any_element(),
            ))
            .child(row(tr!("dialog.cell.new_cell"), cell))
            .children(subgroup.map(note))
            .when(found.orientation_changed, |this| {
                this.child(note(tr!("dialog.symmetrize.orientation")))
            })
            .when(found.origin_shift > 0.0, |this| {
                this.child(note(tr!(
                    "dialog.symmetrize.origin",
                    distance = format::shift(found.origin_shift)
                )))
            })
            .into_any_element()
    }
}

impl Render for SymmetrizeDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let preview = match &self.preview {
            Preview::Searching => h_flex()
                .gap_2()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(tr!("dialog.symmetrize.detecting"))
                .into_any_element(),
            Preview::Found(found) => self.render_found(found, cx),
            Preview::Refused(reason) => div()
                .text_color(theme.danger)
                .child(reason.clone())
                .into_any_element(),
        };
        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(tr!("dialog.symmetrize.tolerance")),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                div()
                                    .w_24()
                                    .flex_none()
                                    .child(Input::new(&self.tolerance).id("symmetrize-tolerance")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(tr!("dialog.symmetrize.tolerance_hint")),
                            ),
                    ),
            )
            .child(
                Checkbox::new("symmetrize-standardize")
                    .label(tr!("dialog.symmetrize.standardize"))
                    .checked(self.standardize)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.set_standardize(*checked, cx)
                    })),
            )
            .child(div().text_sm().child(preview))
    }
}

/// Opens the dialog for the open structure. Returns the dialog's state, for
/// tests and the screenshot tour.
pub(crate) fn open(
    workspace: &StructureWorkspace,
    entity: Entity<StructureWorkspace>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<SymmetrizeDialog> {
    let structure = workspace.document.crystal.snapshot();
    let atoms = workspace.document.derived().atoms_in_cell();
    let dialog = cx.new(|cx| SymmetrizeDialog::new(structure, atoms, window, cx));
    let view = dialog.clone();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (view, entity) = (view.clone(), entity.clone());
        alert
            .title(tr!("dialog.symmetrize.title"))
            .description(tr!("dialog.symmetrize.description"))
            .child(view.clone())
            .ok_text(tr!("dialog.symmetrize.ok"))
            .cancel_text(tr!("dialog.cancel"))
            .show_cancel(true)
            .on_ok(move |_, window, cx| match view.read(cx).edit(cx) {
                Err(reason) => {
                    window.push_notification(Notification::error(reason), cx);
                    false
                }
                Ok(edit) => {
                    entity.update(cx, |workspace, cx| workspace.edit(edit, window, cx).is_ok())
                }
            })
    });
    dialog
}
