//! The side panel and its four tabs (`docs/design/ui-shell.md` §4.3).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal::{Axis, CellParam, Constraint, CoordinateConstraint, CrystalSystem, EditMode};

use crate::actions::ConvertToP1;
use crate::workspace::{PanelTab, StructureWorkspace};
use crate::{dialogs, format};

fn param_name(param: CellParam) -> &'static str {
    match param {
        CellParam::A => "a",
        CellParam::B => "b",
        CellParam::C => "c",
        CellParam::Alpha => "α",
        CellParam::Beta => "β",
        CellParam::Gamma => "γ",
    }
}

fn axis_name(axis: Axis) -> &'static str {
    match axis {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

fn crystal_system_name(system: CrystalSystem) -> SharedString {
    match system {
        CrystalSystem::Triclinic => tr!("crystal_system.triclinic"),
        CrystalSystem::Monoclinic => tr!("crystal_system.monoclinic"),
        CrystalSystem::Orthorhombic => tr!("crystal_system.orthorhombic"),
        CrystalSystem::Tetragonal => tr!("crystal_system.tetragonal"),
        CrystalSystem::Trigonal => tr!("crystal_system.trigonal"),
        CrystalSystem::Hexagonal => tr!("crystal_system.hexagonal"),
        CrystalSystem::Cubic => tr!("crystal_system.cubic"),
    }
}

/// How a field's constraint is shown next to its label.
enum FieldNote {
    None,
    /// Linked to another field, e.g. `= a`.
    Linked(&'static str),
    Fixed,
}

/// A labelled field: the label row carries the constraint icon, and a
/// constrained field is disabled.
fn field(label: SharedString, note: FieldNote, input: Input, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let disabled = !matches!(note, FieldNote::None);
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
            h_flex()
                .gap_1()
                .h_4()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label)
                .map(|row| match note {
                    FieldNote::None => row,
                    FieldNote::Linked(to) => row
                        .child(Icon::new(Lucide::Link).xsmall())
                        .child(format!("= {to}")),
                    FieldNote::Fixed => row.child(Icon::new(Lucide::Lock).xsmall()),
                }),
        )
        .child(input.small().disabled(disabled))
}

fn section(title: impl IntoElement, trailing: Option<SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .p_4()
        .gap_3()
        .border_b_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .justify_between()
                .items_baseline()
                .child(div().text_sm().font_semibold().child(title))
                .children(trailing.map(|text| {
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(text)
                })),
        )
}

impl StructureWorkspace {
    pub(crate) fn render_panel(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let selected = PanelTab::ALL
            .iter()
            .position(|tab| *tab == self.panel)
            .unwrap_or_default();
        let diagnostics = self.document.diagnostics.len();

        v_flex()
            .w(rems(22.))
            .flex_none()
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .child(
                TabBar::new("structure-panel-tabs")
                    .underline()
                    .small()
                    .px_2()
                    .selected_index(selected)
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        if let Some(tab) = PanelTab::ALL.get(*ix) {
                            this.set_panel(*tab, cx);
                        }
                    }))
                    .child(Tab::new().label(tr!("panel.tab.properties")))
                    .child(Tab::new().label(tr!("panel.tab.sites")))
                    .child(Tab::new().label(tr!("panel.tab.measurements")))
                    .child(Tab::new().label(tr!("panel.tab.diagnostics")).when(
                        diagnostics > 0,
                        |tab| {
                            tab.suffix(
                                div()
                                    .ml_1()
                                    .px_1()
                                    .rounded(theme.radius)
                                    .bg(theme.muted)
                                    .text_xs()
                                    .child(diagnostics.to_string()),
                            )
                        },
                    )),
            )
            .child(
                div()
                    .id("structure-panel-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(match self.panel {
                        PanelTab::Properties => {
                            self.render_properties(window, cx).into_any_element()
                        }
                        PanelTab::Sites => self.render_sites(cx).into_any_element(),
                        PanelTab::Measurements => self.render_measurements(cx).into_any_element(),
                        PanelTab::Diagnostics => self.render_diagnostics(cx).into_any_element(),
                    }),
            )
    }

    fn render_properties(&self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let structure = &self.document.structure;
        let derived = &self.document.derived;
        let params = structure.lattice.params();

        let analysis = derived.symmetry.as_ref();
        let cell_field = |param: CellParam| {
            let note = match analysis.map(|a| a.cell.get(param)) {
                None | Some(Constraint::Free) => FieldNote::None,
                Some(Constraint::Equals(other)) => FieldNote::Linked(param_name(other)),
                Some(Constraint::Fixed) => FieldNote::Fixed,
            };
            let input = self
                .cell_inputs
                .iter()
                .find(|(p, _)| *p == param)
                .map(|(_, state)| state.clone());
            let unit = if matches!(param, CellParam::A | CellParam::B | CellParam::C) {
                "Å"
            } else {
                "°"
            };
            input.map(|state| {
                field(
                    param_name(param).into(),
                    note,
                    Input::new(&state).suffix(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(unit),
                    ),
                    cx,
                )
            })
        };

        let cell =
            section(
                tr!("panel.cell.title"),
                Some(analysis.map_or_else(
                    || tr!("panel.cell.system_not_computed"),
                    |a| crystal_system_name(a.crystal_system),
                )),
                cx,
            )
            .child(
                h_flex().gap_2().children(
                    [CellParam::A, CellParam::B, CellParam::C]
                        .into_iter()
                        .filter_map(&cell_field),
                ),
            )
            .child(
                h_flex().gap_2().children(
                    [CellParam::Alpha, CellParam::Beta, CellParam::Gamma]
                        .into_iter()
                        .filter_map(&cell_field),
                ),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                match derived.density {
                    Some(density) => tr!(
                        "panel.cell.summary",
                        volume = format!("{:.2}", params.volume()),
                        density = format!("{density:.2}")
                    ),
                    None => tr!(
                        "panel.cell.volume",
                        volume = format!("{:.2}", params.volume())
                    ),
                },
            ))
            .when(analysis.is_none(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(tr!("panel.cell.constraints_not_computed")),
                )
            });

        let mode = structure.edit_mode();
        let row_label = |label: SharedString| {
            div()
                .w_24()
                .flex_none()
                .text_color(theme.muted_foreground)
                .child(label)
        };
        let space_group = &structure.space_group;
        let space_group_name = match (&space_group.hm_symbol, space_group.it_number) {
            (Some(symbol), Some(number)) => tr!(
                "panel.symmetry.space_group_value",
                symbol = format::space_group(symbol),
                number = number
            ),
            (Some(symbol), None) => format::space_group(symbol).into(),
            (None, Some(number)) => tr!("panel.symmetry.space_group_number", number = number),
            (None, None) => tr!("panel.symmetry.space_group_unnamed"),
        };
        let symmetry = section(tr!("panel.symmetry.title"), None, cx).child(
            v_flex()
                .gap_2()
                .text_sm()
                .child(
                    h_flex()
                        .gap_3()
                        .child(row_label(tr!("panel.symmetry.space_group")))
                        .child(space_group_name),
                )
                .child(
                    h_flex()
                        .gap_3()
                        .child(row_label(tr!("panel.symmetry.operations")))
                        .child(tr!(
                            "panel.symmetry.operation_count",
                            count = structure.symmetry.operations().len()
                        )),
                )
                .child(
                    h_flex()
                        .gap_3()
                        .child(
                            div()
                                .w_24()
                                .flex_none()
                                .text_color(theme.muted_foreground)
                                .child(tr!("panel.symmetry.mode")),
                        )
                        .child(div().flex_1().child(match mode {
                            EditMode::Symmetric => tr!("panel.symmetry.mode_symmetric"),
                            EditMode::P1 => tr!("panel.symmetry.mode_p1"),
                        }))
                        .child(match mode {
                            EditMode::Symmetric => Button::new("convert-to-p1")
                                .outline()
                                .xsmall()
                                .label(tr!("panel.symmetry.convert_to_p1"))
                                .on_click(cx.listener(|_, _, window, cx| {
                                    window.dispatch_action(Box::new(ConvertToP1), cx)
                                })),
                            EditMode::P1 => Button::new("detect-symmetry")
                                .outline()
                                .xsmall()
                                .label(tr!("panel.symmetry.detect"))
                                .on_click(cx.listener(|_, _, window, cx| {
                                    dialogs::not_implemented(
                                        tr!("feature.detect_symmetry"),
                                        window,
                                        cx,
                                    )
                                })),
                        }),
                )
                .when(analysis.is_none(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(tr!("panel.symmetry.name_from_file")),
                    )
                }),
        );

        v_flex()
            .child(cell)
            .child(symmetry)
            .child(self.render_selected_site(cx))
    }

    fn render_selected_site(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let structure = &self.document.structure;
        let derived = &self.document.derived;
        let selected = self.selected_site.and_then(|id| structure.site(id));
        let Some(site) = selected else {
            return v_flex()
                .p_4()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(tr!("panel.site.none"));
        };

        let symmetry = derived.site_symmetry(site.id);
        let coordinate = |axis: usize| {
            let note = match symmetry.map(|s| s.coordinates[axis]) {
                None | Some(CoordinateConstraint::Free) => FieldNote::None,
                Some(CoordinateConstraint::Follows(other)) => FieldNote::Linked(axis_name(other)),
                Some(CoordinateConstraint::Fixed) => FieldNote::Fixed,
            };
            field(
                ["x", "y", "z"][axis].into(),
                note,
                Input::new(&self.coordinate_inputs[axis]),
                cx,
            )
        };
        let element = site
            .occupants
            .first()
            .map(|o| o.species.element.clone())
            .unwrap_or_default();

        section(
            tr!("panel.site.title", label = site.label().to_string()),
            derived
                .multiplicity(site.id)
                .map(|count| tr!("panel.site.atoms", count = count)),
            cx,
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(match symmetry {
                    Some(symmetry) => tr!(
                        "panel.site.wyckoff",
                        wyckoff = symmetry.wyckoff.clone(),
                        form = symmetry.coordinate_form.clone()
                    ),
                    None => tr!("panel.site.wyckoff_not_computed"),
                }),
        )
        .child(h_flex().gap_2().children([0, 1, 2].map(coordinate)))
        .child(
            h_flex()
                .gap_2()
                .child(
                    v_flex()
                        .flex_1()
                        .gap_1()
                        .child(
                            div()
                                .h_4()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(tr!("panel.site.element")),
                        )
                        .child(
                            Button::new("site-element")
                                .outline()
                                .small()
                                .w_full()
                                .child(
                                    h_flex().w_full().justify_between().child(element).child(
                                        Icon::new(IconName::ChevronDown)
                                            .xsmall()
                                            .text_color(theme.muted_foreground),
                                    ),
                                )
                                .on_click(cx.listener(|_, _, window, cx| {
                                    dialogs::not_implemented(
                                        tr!("feature.change_element"),
                                        window,
                                        cx,
                                    )
                                })),
                        ),
                )
                .child(field(
                    tr!("panel.site.occupancy"),
                    FieldNote::None,
                    Input::new(&self.occupancy_input),
                    cx,
                )),
        )
        .child(
            h_flex().child(
                Button::new("add-occupant")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label(tr!("panel.site.add_occupant"))
                    .on_click(cx.listener(|_, _, window, cx| {
                        dialogs::not_implemented(tr!("feature.mixed_occupancy"), window, cx)
                    })),
            ),
        )
    }

    fn render_sites(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let structure = &self.document.structure;
        let rows = structure.sites.iter().map(|site| {
            let id = site.id;
            let derived = &self.document.derived;
            let wyckoff = derived.site_symmetry(id).map(|s| s.wyckoff.clone());
            let count = derived.multiplicity(id).unwrap_or_default();
            let occupant = site.occupants.first();
            let element = occupant
                .map(|o| o.species.element.clone())
                .unwrap_or_default();
            let occupancy = occupant.map_or(1.0, |o| o.occupancy.value());
            let coordinates = site.position.map(format::coordinate).join(", ");
            ListItem::new(("site", id.0 as usize))
                .selected(self.selected_site == Some(id))
                .on_click(cx.listener(move |this, _, window, cx| this.select_site(id, window, cx)))
                .child(
                    v_flex()
                        .w_full()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().font_medium().child(site.label().to_string()))
                                .child(div().text_xs().text_color(theme.muted_foreground).child(
                                    match wyckoff {
                                        Some(wyckoff) => tr!(
                                            "panel.sites.row",
                                            element = element.clone(),
                                            wyckoff = wyckoff,
                                            count = count
                                        ),
                                        None => tr!(
                                            "panel.sites.row_without_wyckoff",
                                            element = element.clone(),
                                            count = count
                                        ),
                                    },
                                )),
                        )
                        .child(
                            h_flex()
                                .justify_between()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                                .child(coordinates)
                                .child(format::occupancy(occupancy)),
                        ),
                )
        });

        v_flex()
            .p_2()
            .gap_1()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(tr!("panel.sites.title")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(tr!("panel.sites.count", count = structure.sites.len())),
                    )
                    .child(
                        Button::new("add-site")
                            .outline()
                            .xsmall()
                            .icon(IconName::Plus)
                            .label(tr!("panel.sites.add"))
                            .on_click(cx.listener(|_, _, window, cx| {
                                dialogs::not_implemented(tr!("feature.add_site"), window, cx)
                            })),
                    ),
            )
            .children(rows)
            .when(structure.edit_mode() == EditMode::Symmetric, |this| {
                this.child(
                    div()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(tr!("panel.sites.symmetric_hint")),
                )
            })
    }

    fn render_measurements(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let rows = self.document.measurements.iter().map(|m| {
            h_flex()
                .justify_between()
                .px_2()
                .py_1p5()
                .text_sm()
                .child(m.label.clone())
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .child(format!("{:.3} Å", m.distance())),
                )
        });
        v_flex()
            .p_2()
            .gap_1()
            .children(rows)
            .when(self.document.measurements.is_empty(), |this| {
                this.child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(tr!("panel.measurements.empty")),
                )
            })
            .child(
                div()
                    .px_2()
                    .pt_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(tr!("panel.measurements.hint")),
            )
    }

    fn render_diagnostics(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .p_2()
            .gap_1()
            .children(self.document.diagnostics.iter().map(|d| d.row()))
            .when(self.document.diagnostics.is_empty(), |this| {
                this.child(
                    div()
                        .p_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(tr!("panel.diagnostics.empty")),
                )
            })
    }
}
