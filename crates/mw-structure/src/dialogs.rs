//! Dialogs for structure commands: the choices when a deletion would break
//! the symmetry (design §7.1), adding a site or a mixed occupant (design
//! §7.6–7.7), converting to P1 and making a supercell (design §7.9).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::*;
use mw_crystal::{EditError, EditMode, SiteId, Target, special_position};

use crate::workspace::{StructureWorkspace, element_picker};
use crate::{editing, format};

/// Å. A typed position this close to its special position counts as on
/// it; farther, the dialog says it will snap. The importer and the edit
/// commands report snapping from the same distance.
const REPORTED_SNAP: f64 = 1e-4;

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
    targets: Vec<Target>,
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
        let one_entity = entity.clone();
        let sites = sites.clone();
        let targets = targets.clone();
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
                .child(Button::new("delete-one").primary().label(one).on_click(
                    move |_, window, cx| {
                        window.close_dialog(cx);
                        one_entity.update(cx, |workspace, cx| {
                            workspace.delete_after_expanding(targets.clone(), window, cx)
                        });
                    },
                )),
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
                                .id("add-occupant-element")
                                .placeholder(tr!("dialog.add_occupant.choose_element"))
                                .menu_max_h(rems(16.))
                                .into_any_element(),
                        )),
                    )
                    .child(
                        div().w_32().child(field(
                            tr!("panel.site.occupancy"),
                            Input::new(&occupancy)
                                .id("add-occupant-occupancy")
                                .into_any_element(),
                        )),
                    ),
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

/// Adds a site at typed fractional coordinates (design §7.6). The dialog
/// tells as they are typed where the site will snap and how many atoms it
/// will have. A position on an existing site's atoms offers a mixed
/// occupant there instead.
pub(crate) fn open_add_site(
    workspace: &StructureWorkspace,
    entity: Entity<StructureWorkspace>,
    window: &mut Window,
    cx: &mut App,
) {
    let structure = workspace.document.crystal.snapshot();
    let element = element_picker("", window, cx);
    let coordinates: [Entity<InputState>; 3] =
        [0, 1, 2].map(|_| cx.new(|cx| InputState::new(window, cx).placeholder("0")));
    let occupancy = cx.new(|cx| InputState::new(window, cx).default_value("1"));
    window.open_alert_dialog(cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        let field = |text: SharedString, input: AnyElement| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().text_xs().text_color(muted).child(text))
                .child(input)
        };
        let typed = || -> Option<[f64; 3]> {
            let mut position = [0.0_f64; 3];
            for (value, input) in position.iter_mut().zip(&coordinates) {
                *value = input.read(cx).value().trim().parse().ok()?;
            }
            position.iter().all(|v| v.is_finite()).then_some(position)
        };
        let summary = match typed() {
            Some(position) => {
                let special = special_position(&structure, position);
                let count = structure.symmetry.operations().len() / special.order.max(1);
                let form = special.form.to_string();
                if special.displacement >= REPORTED_SNAP {
                    tr!(
                        "dialog.add_site.snaps",
                        form = form,
                        distance = format::distance(special.displacement),
                        count = count
                    )
                } else {
                    tr!("dialog.add_site.atoms", form = form, count = count)
                }
            }
            None => tr!("dialog.add_site.hint"),
        };
        let coordinate = |axis: usize| {
            let name = ["x", "y", "z"][axis];
            field(
                name.into(),
                Input::new(&coordinates[axis])
                    .id(SharedString::from(format!("add-site-{name}")))
                    .into_any_element(),
            )
        };
        let (element, coordinates, occupancy, entity) = (
            element.clone(),
            coordinates.clone(),
            occupancy.clone(),
            entity.clone(),
        );
        dialog
            .title(tr!("dialog.add_site.title"))
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(field(
                                tr!("panel.site.element"),
                                Select::new(&element)
                                    .id("add-site-element")
                                    .placeholder(tr!("dialog.add_occupant.choose_element"))
                                    .menu_max_h(rems(16.))
                                    .into_any_element(),
                            ))
                            .child(
                                div().w_32().child(field(
                                    tr!("panel.site.occupancy"),
                                    Input::new(&occupancy)
                                        .id("add-site-occupancy")
                                        .into_any_element(),
                                )),
                            ),
                    )
                    .child(h_flex().gap_2().children([0, 1, 2].map(coordinate)))
                    .child(div().text_xs().text_color(muted).child(summary)),
            )
            .ok_text(tr!("dialog.add_site.ok"))
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
                let mut position = [0.0; 3];
                for (value, input) in position.iter_mut().zip(&coordinates) {
                    let text = input.read(cx).value();
                    let Some(number) = editing::parse_number(&text, window, cx) else {
                        return false;
                    };
                    *value = number;
                }
                let text = occupancy.read(cx).value();
                let Some(value) = editing::parse_number(&text, window, cx) else {
                    return false;
                };
                // On another site's atoms, the offer of a mixed occupant
                // opens once this dialog has closed: one dialog at a time.
                entity.update(cx, |workspace, cx| {
                    workspace.add_site_at(position, symbol, value, window, cx)
                })
            })
    });
}

/// A new site would coincide with atoms of `site`: offers adding the
/// element to that site as a mixed occupant instead (design §7.6).
pub(crate) fn open_coincident_site(
    entity: Entity<StructureWorkspace>,
    site: SiteId,
    element: &'static str,
    occupancy: f64,
    window: &mut Window,
    cx: &mut App,
) {
    let label = entity
        .read(cx)
        .document
        .structure()
        .site(site)
        .map_or_else(String::new, |s| s.label().to_string());
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let entity = entity.clone();
        dialog
            .title(tr!("dialog.coincident_site.title", label = label.clone()))
            .description(tr!(
                "dialog.coincident_site.description",
                label = label.clone(),
                element = element
            ))
            .ok_text(tr!("dialog.coincident_site.ok"))
            .cancel_text(tr!("dialog.cancel"))
            .show_cancel(true)
            .on_ok(move |_, window, cx| {
                entity.update(cx, |workspace, cx| {
                    let added = workspace.add_occupant(site, element, occupancy, window, cx);
                    if added {
                        workspace.select_site(site, window, cx);
                    }
                    added
                })
            })
    });
}

pub(crate) fn open_convert_to_p1(
    workspace: &StructureWorkspace,
    entity: Entity<StructureWorkspace>,
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
            .on_ok({
                let entity = entity.clone();
                move |_, window, cx| {
                    entity.update(cx, |workspace, cx| workspace.expand_to_p1(window, cx));
                    true
                }
            })
    });
}

/// The largest repeat along one axis the supercell dialog accepts.
const MAX_REPEAT: i32 = 20;
/// Supercells with more atoms are slow to show; the dialog says so.
const LARGE_SUPERCELL: usize = 100_000;

/// Asks how many times to repeat the cell along a, b and c, then makes the
/// supercell as a new structure (design §7.9). The matrix is diagonal; the
/// command takes any integer matrix.
pub(crate) fn open_supercell(
    workspace: &StructureWorkspace,
    entity: Entity<StructureWorkspace>,
    window: &mut Window,
    cx: &mut App,
) {
    let name = workspace.document.name.to_string();
    let atoms = workspace.document.derived().atoms_in_cell();
    let inputs: [Entity<InputState>; 3] =
        [0, 1, 2].map(|_| cx.new(|cx| InputState::new(window, cx).default_value("2")));
    window.open_alert_dialog(cx, move |dialog, _, cx| {
        let repeats = || -> Option<[i32; 3]> {
            let mut repeats = [0; 3];
            for (repeat, input) in repeats.iter_mut().zip(&inputs) {
                *repeat = input.read(cx).value().trim().parse().ok()?;
                if !(1..=MAX_REPEAT).contains(repeat) {
                    return None;
                }
            }
            Some(repeats)
        };
        let summary = match repeats() {
            Some([a, b, c]) => {
                let total = atoms * (a * b * c) as usize;
                if total > LARGE_SUPERCELL {
                    tr!("dialog.supercell.large", count = total)
                } else {
                    tr!("dialog.supercell.count", count = total)
                }
            }
            None => tr!("dialog.supercell.invalid", max = MAX_REPEAT),
        };
        let field = |axis: &'static str, input: &Entity<InputState>| {
            v_flex()
                .flex_1()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(axis),
                )
                .child(Input::new(input).id(SharedString::from(format!("supercell-{axis}"))))
        };
        let times = || {
            div()
                .pt_5()
                .text_color(cx.theme().muted_foreground)
                .child("×")
        };
        let inputs = inputs.clone();
        let entity = entity.clone();
        dialog
            .title(tr!("dialog.supercell.title"))
            .description(tr!("dialog.supercell.description", name = name.clone()))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(field("a", &inputs[0]))
                            .child(times())
                            .child(field("b", &inputs[1]))
                            .child(times())
                            .child(field("c", &inputs[2])),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(summary),
                    ),
            )
            .ok_text(tr!("dialog.supercell.ok"))
            .cancel_text(tr!("dialog.cancel"))
            .show_cancel(true)
            .on_ok(move |_, window, cx| {
                let mut repeats = [0; 3];
                for (repeat, input) in repeats.iter_mut().zip(&inputs) {
                    match input.read(cx).value().trim().parse::<i32>() {
                        Ok(value) if (1..=MAX_REPEAT).contains(&value) => *repeat = value,
                        _ => {
                            window.push_notification(
                                Notification::error(tr!(
                                    "dialog.supercell.invalid",
                                    max = MAX_REPEAT
                                )),
                                cx,
                            );
                            return false;
                        }
                    }
                }
                entity.update(cx, |workspace, cx| {
                    workspace.make_supercell_of(repeats, window, cx)
                });
                true
            })
    });
}
