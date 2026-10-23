//! Dialogs for structure commands: the choices when a deletion would break
//! the symmetry (design §7.1) and adding a mixed occupant. Converting to P1
//! and making supercells show their final wording, but their confirm
//! buttons only report that the operation is not implemented yet (M5).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::*;
use mw_crystal::{EditError, EditMode, SiteId};

use crate::workspace::{StructureWorkspace, element_picker};
use crate::{editing, format};

/// Mock: tells the user a command exists but does nothing yet.
pub(crate) fn not_implemented(feature: SharedString, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::info(tr!("mock.not_implemented", feature = feature.to_string())),
        cx,
    );
}

/// Deleting some but not all equivalent atoms of a site would break the
/// symmetry: offers deleting the whole sites the selection touches, or
/// converting to P1 first (design §7.1).
pub(crate) fn open_partial_delete(
    workspace: &StructureWorkspace,
    entity: Entity<StructureWorkspace>,
    error: &EditError,
    sites: Vec<SiteId>,
    window: &mut Window,
    cx: &mut App,
) {
    let EditError::BreaksSymmetry {
        site, equivalent, ..
    } = *error
    else {
        return;
    };
    let document = &workspace.document;
    let label_of = |site: SiteId| {
        document
            .structure()
            .site(site)
            .map_or_else(String::new, |s| s.label().to_string())
    };
    let label = label_of(site);
    let labels: Vec<String> = sites.iter().map(|&site| label_of(site)).collect();
    let all: usize = sites
        .iter()
        .map(|&site| document.derived().multiplicity(site).unwrap_or(0))
        .sum();
    let symbol: SharedString = match document.space_group_symbol() {
        Some(symbol) => symbol.into(),
        None => tr!(
            "dialog.delete_atom.operations",
            count = document.structure().symmetry.operations().len()
        ),
    };
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let entity = entity.clone();
        let sites = sites.clone();
        // The selection can touch several sites; the texts then name them
        // all, as the deletion takes them all.
        let (title, description, one) = match &labels[..] {
            [_] => (
                tr!("dialog.delete_atom.title", label = label.clone()),
                tr!(
                    "dialog.delete_atom.description",
                    symbol = symbol.clone(),
                    label = label.clone(),
                    count = equivalent
                ),
                tr!("dialog.delete_atom.one"),
            ),
            _ => (
                tr!("dialog.delete_atom.title_many"),
                tr!(
                    "dialog.delete_atom.description_many",
                    symbol = symbol.clone(),
                    labels = labels.join(&tr!("dialog.list_separator")),
                    count = all
                ),
                tr!("dialog.delete_atom.one_many"),
            ),
        };
        dialog.title(title).description(description).footer(
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
                        .label(tr!("dialog.delete_atom.all", count = all))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            entity.update(cx, |workspace, cx| {
                                workspace.delete_sites(sites.clone(), window, cx)
                            });
                        }),
                )
                .child(
                    Button::new("delete-one")
                        .primary()
                        .label(one)
                        .on_click(|_, window, cx| {
                            window.close_dialog(cx);
                            // Mock: converting to P1 is M5.
                            not_implemented(tr!("feature.convert_to_p1"), window, cx);
                        }),
                ),
        )
    });
}

/// Adds an element to the edited site, making its occupancy mixed. The
/// occupancy starts at what the site has left, if anything.
pub(crate) fn open_add_occupant(
    workspace: &StructureWorkspace,
    entity: Entity<StructureWorkspace>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(site) = workspace
        .editing_site
        .and_then(|id| workspace.document.structure().site(id))
    else {
        return;
    };
    let (site_id, label) = (site.id, site.label().to_string());
    let free = 1.0
        - site
            .occupants
            .iter()
            .map(|o| o.occupancy.value())
            .sum::<f64>();
    let element = element_picker("", window, cx);
    let occupancy = cx.new(|cx| {
        let state = InputState::new(window, cx).placeholder("0.1");
        if free > 0.0005 {
            state.default_value(format::occupancy(free))
        } else {
            state
        }
    });
    window.open_alert_dialog(cx, move |dialog, _, cx| {
        let field = |text: SharedString, input: AnyElement| {
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(text),
                )
                .child(input)
        };
        let (element, occupancy, entity) = (element.clone(), occupancy.clone(), entity.clone());
        dialog
            .title(tr!("dialog.add_occupant.title", label = label.clone()))
            .description(tr!("dialog.add_occupant.description"))
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        div().flex_1().child(field(
                            tr!("panel.site.element"),
                            Select::new(&element)
                                .placeholder(tr!("dialog.add_occupant.choose_element"))
                                .menu_max_h(rems(16.))
                                .into_any_element(),
                        )),
                    )
                    .child(div().w_32().child(field(
                        tr!("panel.site.occupancy"),
                        Input::new(&occupancy).into_any_element(),
                    ))),
            )
            .ok_text(tr!("dialog.add_occupant.ok"))
            .cancel_text(tr!("dialog.cancel"))
            .show_cancel(true)
            .on_ok(move |_, window, cx| {
                let Some(symbol) = element.read(cx).selected_value().copied() else {
                    window.push_notification(
                        Notification::error(tr!("dialog.add_occupant.no_element")),
                        cx,
                    );
                    return false;
                };
                let text = occupancy.read(cx).value();
                let Some(value) = editing::parse_number(&text, window, cx) else {
                    return false;
                };
                entity.update(cx, |workspace, cx| {
                    workspace.add_occupant(site_id, symbol, value, window, cx)
                })
            })
    });
}

pub(crate) fn open_convert_to_p1(
    workspace: &StructureWorkspace,
    window: &mut Window,
    cx: &mut App,
) {
    let document = &workspace.document;
    if document.structure().edit_mode() == EditMode::P1 {
        window.push_notification(Notification::info(tr!("dialog.convert.already_p1")), cx);
        return;
    }
    let sites = document.structure().sites.len();
    let atoms = document.derived().atoms_in_cell();
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
