//! Dialogs and mock notices for structure commands. The dialogs show the
//! final wording and choices (design §7.1); their confirm buttons only
//! report that the operation is not implemented yet.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{WindowExt, h_flex};
use gpui_kit::*;
use mw_crystal::EditMode;

use crate::workspace::StructureWorkspace;

/// Mock: tells the user a command exists but does nothing yet.
pub(crate) fn not_implemented(feature: SharedString, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::info(tr!("mock.not_implemented", feature = feature.to_string())),
        cx,
    );
}

pub(crate) fn open_delete(workspace: &StructureWorkspace, window: &mut Window, cx: &mut App) {
    let structure = &workspace.document.structure;
    let Some(site) = workspace.selected_site().and_then(|id| structure.site(id)) else {
        return;
    };
    let label = site.label().to_string();
    let count = workspace
        .document
        .derived
        .multiplicity(site.id)
        .unwrap_or(1);
    let symbol: SharedString = match workspace.document.space_group_symbol() {
        Some(symbol) => symbol.into(),
        None => tr!(
            "dialog.delete_atom.operations",
            count = structure.symmetry.operations().len()
        ),
    };

    if structure.edit_mode() == EditMode::P1 || count == 1 {
        window.open_alert_dialog(cx, move |dialog, _, _| {
            dialog
                .title(tr!("dialog.delete_site.title", label = label.clone()))
                .description(tr!("dialog.delete_site.description"))
                .ok_text(tr!("dialog.delete"))
                .cancel_text(tr!("dialog.cancel"))
                .show_cancel(true)
                .on_ok(|_, window, cx| {
                    not_implemented(tr!("feature.delete"), window, cx);
                    true
                })
        });
        return;
    }

    window.open_alert_dialog(cx, move |dialog, _, _| {
        dialog
            .title(tr!("dialog.delete_atom.title", label = label.clone()))
            .description(tr!(
                "dialog.delete_atom.description",
                symbol = symbol.clone(),
                label = label.clone(),
                count = count
            ))
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("delete-cancel")
                            .label(tr!("dialog.cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("delete-orbit")
                            .label(tr!("dialog.delete_atom.all", count = count))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                not_implemented(tr!("feature.delete"), window, cx);
                            }),
                    )
                    .child(
                        Button::new("delete-one")
                            .primary()
                            .label(tr!("dialog.delete_atom.one"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                not_implemented(tr!("feature.convert_to_p1"), window, cx);
                            }),
                    ),
            )
    });
}

pub(crate) fn open_convert_to_p1(
    workspace: &StructureWorkspace,
    window: &mut Window,
    cx: &mut App,
) {
    let document = &workspace.document;
    if document.structure.edit_mode() == EditMode::P1 {
        window.push_notification(Notification::info(tr!("dialog.convert.already_p1")), cx);
        return;
    }
    let sites = document.structure.sites.len();
    let atoms = document.derived.atoms_in_cell();
    window.open_alert_dialog(cx, move |dialog, _, _| {
        dialog
            .title(tr!("dialog.convert.title"))
            .description(tr!(
                "dialog.convert.description",
                sites = sites,
                atoms = atoms
            ))
            .ok_text(tr!("dialog.convert.ok"))
            .cancel_text(tr!("dialog.cancel"))
            .show_cancel(true)
            .on_ok(|_, window, cx| {
                not_implemented(tr!("feature.convert_to_p1"), window, cx);
                true
            })
    });
}

pub(crate) fn open_supercell(workspace: &StructureWorkspace, window: &mut Window, cx: &mut App) {
    let name = workspace.document.name.to_string();
    window.open_alert_dialog(cx, move |dialog, _, _| {
        dialog
            .title(tr!("dialog.supercell.title"))
            .description(tr!("dialog.supercell.description", name = name.clone()))
            .ok_text(tr!("dialog.supercell.ok"))
            .cancel_text(tr!("dialog.cancel"))
            .show_cancel(true)
            .on_ok(|_, window, cx| {
                not_implemented(tr!("feature.supercell"), window, cx);
                true
            })
    });
}
