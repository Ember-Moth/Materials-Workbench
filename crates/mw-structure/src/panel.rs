//! The side panel and its four tabs (`docs/design/ui-shell.md` §4.3). The
//! measurements tab lives in `measure.rs`.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::select::Select;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal::{
    Axis, CELL_DEVIATION, CellParam, CellParams, Constraint, CoordinateConstraint, CrystalSystem,
    EditMode, Orientation, Setting, SpaceGroupType, Symmetry, cell_deviation,
};

use mw_project_file::{Operation, Provenance};

use crate::actions::{
    AddMixedOccupant, AddSite, AlignCell, ConvertToP1, Symmetrize, TransformCell,
};
use crate::disorder::disorder_code;
use crate::format;
use crate::selection::Selection;
use crate::workspace::{PanelTab, StructureWorkspace};

/// What the cell section says about an explicit orientation (crystallography
/// design §4.2); nothing for the standard one.
fn orientation_note(orientation: &Orientation) -> Option<SharedString> {
    match orientation {
        Orientation::Standard => None,
        _ if orientation.is_left_handed() => Some(tr!("panel.cell.left_handed")),
        Orientation::Explicit(_) => Some(tr!("panel.cell.oriented")),
    }
}

/// What the cell section says about a cell off its symmetry by more than
/// rounding; nothing for a cell that keeps it.
pub(crate) fn cell_deviation_note(
    symmetry: &Symmetry,
    params: &CellParams,
) -> Option<SharedString> {
    let (length, angle) = cell_deviation(symmetry, params);
    (length > CELL_DEVIATION || angle > CELL_DEVIATION).then(|| {
        tr!(
            "panel.cell.off_symmetry",
            length = format!("{length:.4}"),
            angle = format!("{angle:.3}")
        )
    })
}

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

/// The space group type and its number, e.g. `P4₂/mnm (No. 136)`.
pub(crate) fn space_group_name(number: u16, symbol: &str) -> SharedString {
    tr!(
        "panel.symmetry.space_group_value",
        symbol = format::space_group(symbol),
        number = number
    )
}

/// The cell parameter as an identifier, e.g. `alpha`.
fn param_id(param: CellParam) -> &'static str {
    match param {
        CellParam::A => "a",
        CellParam::B => "b",
        CellParam::C => "c",
        CellParam::Alpha => "alpha",
        CellParam::Beta => "beta",
        CellParam::Gamma => "gamma",
    }
}

/// A field's identity from what it edits, e.g. `cell-a` or `occupancy-Ti1`.
pub(crate) fn field_id(kind: &str, name: &str) -> ElementId {
    ElementId::Name(format!("{kind}-{name}").into())
}

/// How a field's constraint is shown next to its label.
enum FieldNote {
    None,
    /// Follows another field and can be typed into, which changes that one
    /// too, e.g. `= 2x` (design §7.5).
    Follows(SharedString),
    /// Linked to other fields and set through them, e.g. `= a`.
    Linked(SharedString),
    Fixed,
}

/// A labelled field: the label row carries the constraint icon, and a
/// field set through others, or fixed, is disabled.
fn field(label: SharedString, note: FieldNote, input: Input, cx: &App) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let disabled = matches!(note, FieldNote::Linked(_) | FieldNote::Fixed);
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
                    FieldNote::Follows(to) | FieldNote::Linked(to) => row
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
        let structure = self.document.structure();
        let derived = self.document.derived();
        let params = structure.lattice.params();

        let analysis = &derived.symmetry;
        let cell_field = |param: CellParam| {
            let note = match analysis.cell.get(param) {
                Constraint::Free => FieldNote::None,
                Constraint::Equals(other) => FieldNote::Linked(param_name(other).into()),
                Constraint::Fixed => FieldNote::Fixed,
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
                    Input::new(&state)
                        .id(field_id("cell", param_id(param)))
                        .suffix(
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
                Some(
                    analysis
                        .crystal_system()
                        .map_or_else(|| tr!("panel.cell.system_unknown"), crystal_system_name),
                ),
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
            // A cell kept as a file or a change of cell gave it looks turned
            // against the standard orientation; say why.
            .when_some(
                orientation_note(structure.lattice.orientation()),
                |this, text| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(text),
                    )
                },
            )
            // An imported cell a little off its symmetry, which the importer
            // warned about (crystallography design §7.3): how far, and the
            // one step that aligns it.
            .when_some(
                cell_deviation_note(&structure.symmetry, params),
                |this, text| {
                    this.child(
                        h_flex()
                            .gap_2()
                            .items_start()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(text),
                            )
                            .child(
                                Button::new("align-cell")
                                    .outline()
                                    .xsmall()
                                    .label(tr!("panel.cell.align"))
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        window.dispatch_action(Box::new(AlignCell), cx)
                                    })),
                            ),
                    )
                },
            );

        let mode = structure.edit_mode();
        let row_label = |label: SharedString| {
            div()
                .w_24()
                .flex_none()
                .text_color(theme.muted_foreground)
                .child(label)
        };
        let row = |label: SharedString, value: SharedString| {
            h_flex()
                .gap_3()
                .child(row_label(label))
                .child(div().flex_1().min_w_0().child(value))
        };
        let note = |text: SharedString| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
        };
        let space_group = analysis.space_group.as_ref();
        let setting = space_group.and_then(|sg| match &sg.setting {
            Setting::Standard => None,
            Setting::Tabulated { symbol, code } => Some(tr!(
                "panel.symmetry.setting_value",
                symbol = format::space_group(symbol),
                code = *code
            )),
            Setting::Transformed => Some(tr!("panel.symmetry.setting_transformed")),
        });
        // In P1, what the atoms' own symmetry is, when it is higher.
        let detected = analysis
            .detected
            .as_ref()
            .filter(|d| mode == EditMode::P1 && d.number > 1);
        let symmetry = section(tr!("panel.symmetry.title"), None, cx).child(
            v_flex()
                .gap_2()
                .text_sm()
                .child(
                    h_flex()
                        .gap_3()
                        .child(row_label(tr!("panel.symmetry.space_group")))
                        .child(div().flex_1().min_w_0().child(space_group.map_or_else(
                            || tr!("panel.symmetry.space_group_unknown"),
                            |sg: &SpaceGroupType| space_group_name(sg.number, sg.hm_symbol),
                        )))
                        .child(
                            Button::new("transform-cell")
                                .outline()
                                .xsmall()
                                .label(tr!("panel.symmetry.transform_cell"))
                                .on_click(cx.listener(|_, _, window, cx| {
                                    window.dispatch_action(Box::new(TransformCell), cx)
                                })),
                        ),
                )
                .children(setting.map(|setting| row(tr!("panel.symmetry.setting"), setting)))
                .child(row(
                    tr!("panel.symmetry.operations"),
                    tr!(
                        "panel.symmetry.operation_count",
                        count = structure.symmetry.operations().len()
                    ),
                ))
                .children(detected.map(|d| {
                    row(
                        tr!("panel.symmetry.detected"),
                        space_group_name(d.number, d.hm_symbol),
                    )
                }))
                .child(
                    h_flex()
                        .gap_3()
                        .child(row_label(tr!("panel.symmetry.mode")))
                        .child(div().flex_1().child(match mode {
                            EditMode::Symmetric => tr!("panel.symmetry.mode_symmetric"),
                            EditMode::P1 => tr!("panel.symmetry.mode_p1"),
                        }))
                        .map(|this| match mode {
                            EditMode::Symmetric => this.child(
                                Button::new("convert-to-p1")
                                    .outline()
                                    .xsmall()
                                    .label(tr!("panel.symmetry.convert_to_p1"))
                                    .on_click(cx.listener(|_, _, window, cx| {
                                        window.dispatch_action(Box::new(ConvertToP1), cx)
                                    })),
                            ),
                            EditMode::P1 => this.when(detected.is_some(), |this| {
                                this.child(
                                    Button::new("symmetrize")
                                        .outline()
                                        .xsmall()
                                        .label(tr!("panel.symmetry.symmetrize"))
                                        .on_click(cx.listener(|_, _, window, cx| {
                                            window.dispatch_action(Box::new(Symmetrize), cx)
                                        })),
                                )
                            }),
                        }),
                )
                .when(
                    matches!(
                        space_group.map(|sg| &sg.setting),
                        Some(Setting::Transformed)
                    ),
                    |this| this.child(note(tr!("panel.symmetry.transformed_hint"))),
                )
                .when(detected.is_some(), |this| {
                    this.child(note(tr!("panel.symmetry.detected_hint")))
                }),
        );

        let source = section(tr!("panel.source.title"), None, cx)
            .child(div().text_sm().child(self.source_text()));

        v_flex()
            .child(cell)
            .child(symmetry)
            .child(self.render_selected_site(cx))
            .child(source)
    }

    /// Where the structure came from (`docs/design/project-format.md` §7).
    fn source_text(&self) -> SharedString {
        match &self.document.provenance {
            Provenance::Import(import) => {
                tr!("panel.source.import", file = import.file_name.clone())
            }
            Provenance::Derived(derived) => match derived.operation {
                Operation::MakeSupercell { matrix } => {
                    let diagonal = (0..3).all(|i| (0..3).all(|j| i == j || matrix[i][j] == 0));
                    if diagonal {
                        tr!(
                            "panel.source.supercell",
                            name = derived.from_name.clone(),
                            a = matrix[0][0],
                            b = matrix[1][1],
                            c = matrix[2][2]
                        )
                    } else {
                        let rows: Vec<String> = matrix
                            .iter()
                            .map(|row| format!("{} {} {}", row[0], row[1], row[2]))
                            .collect();
                        tr!(
                            "panel.source.supercell_matrix",
                            name = derived.from_name.clone(),
                            matrix = rows.join(" / ")
                        )
                    }
                }
                _ => tr!("panel.source.derived", name = derived.from_name.clone()),
            },
            _ => tr!("panel.source.created"),
        }
    }

    fn render_selected_site(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let structure = self.document.structure();
        let derived = self.document.derived();
        let selected = self.selected_site().and_then(|id| structure.site(id));
        let Some(site) = selected else {
            return v_flex()
                .p_4()
                .border_b_1()
                .border_color(theme.border)
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(tr!("panel.site.none"));
        };

        let symmetry = derived.site_symmetry(site.id);
        let coordinate = |axis: usize| {
            let note = match symmetry.map(|s| (s, s.coordinates[axis])) {
                None | Some((_, CoordinateConstraint::Free)) => FieldNote::None,
                Some((s, CoordinateConstraint::Follows(_))) => {
                    FieldNote::Follows(s.form.expression(Axis::ALL[axis]).into())
                }
                Some((s, CoordinateConstraint::Combination)) => {
                    FieldNote::Linked(s.form.expression(Axis::ALL[axis]).into())
                }
                Some((_, CoordinateConstraint::Fixed)) => FieldNote::Fixed,
            };
            field(
                ["x", "y", "z"][axis].into(),
                note,
                Input::new(&self.coordinate_inputs[axis])
                    .id(field_id("coordinate", ["x", "y", "z"][axis])),
                cx,
            )
        };
        let site_id = site.id;
        let mixed = self.occupants.len() > 1;
        let column = |text: SharedString| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
        };
        // Label, element and occupancy line up in three columns.
        let occupants = self.occupants.iter().map(|editor| {
            let label = editor.label.clone();
            h_flex()
                .gap_2()
                .child(
                    div().w_20().flex_none().child(
                        Input::new(&editor.label_input)
                            .id(field_id("label", &label))
                            .small(),
                    ),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        Select::new(&editor.element)
                            .id(field_id("element", &label))
                            .small()
                            .menu_max_h(rems(16.))
                            .search_placeholder(tr!("panel.site.search_element")),
                    ),
                )
                .child(
                    div().w_20().flex_none().child(
                        Input::new(&editor.occupancy)
                            .id(field_id("occupancy", &label))
                            .small(),
                    ),
                )
                .when(mixed, |row| {
                    row.child(
                        Button::new(ElementId::Name(format!("remove-{label}").into()))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(tr!("panel.site.remove_occupant", label = label.clone()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_occupant(site_id, label.clone(), window, cx)
                            })),
                    )
                })
        });

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
                .children(symmetry.map(|symmetry| match &symmetry.wyckoff {
                    Some(wyckoff) => tr!(
                        "panel.site.wyckoff",
                        wyckoff = wyckoff.to_string(),
                        symmetry = wyckoff.site_symmetry,
                        form = symmetry.form.to_string()
                    ),
                    None => tr!("panel.site.form", form = symmetry.form.to_string()),
                })),
        )
        .child(h_flex().gap_2().children([0, 1, 2].map(coordinate)))
        .child(
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .w_20()
                                .flex_none()
                                .child(column(tr!("panel.site.label"))),
                        )
                        .child(div().flex_1().child(column(tr!("panel.site.element"))))
                        .child(
                            div()
                                .w_20()
                                .flex_none()
                                .child(column(tr!("panel.site.occupancy"))),
                        )
                        // Room for the remove buttons.
                        .when(mixed, |row| row.child(div().w_6().flex_none())),
                )
                .children(occupants),
        )
        .child(
            h_flex().child(
                Button::new("add-occupant")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .label(tr!("panel.site.add_occupant"))
                    .on_click(cx.listener(|_, _, window, cx| {
                        window.dispatch_action(Box::new(AddMixedOccupant), cx)
                    })),
            ),
        )
        .child(self.render_disorder_fields(cx))
    }

    /// The disorder of the selected sites: assembly and group, and the
    /// button that takes them out of disorder (crystallography design
    /// §7.2), with notes on what the fields need and change.
    fn render_disorder_fields(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let notes = self.disorder_notes(cx);
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .child(field(
                        tr!("panel.site.disorder_assembly"),
                        FieldNote::None,
                        Input::new(&self.disorder.assembly).id("disorder-assembly"),
                        cx,
                    ))
                    .child(field(
                        tr!("panel.site.disorder_group"),
                        FieldNote::None,
                        Input::new(&self.disorder.group).id("disorder-group"),
                        cx,
                    ))
                    .when(self.selection_has_disorder(), |row| {
                        row.child(
                            Button::new("clear-disorder")
                                .ghost()
                                .small()
                                .icon(IconName::Close)
                                .tooltip(tr!("panel.site.clear_disorder"))
                                .accessibility_label(tr!("panel.site.clear_disorder"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_disorder(window, cx)
                                })),
                        )
                    }),
            )
            .children(notes.into_iter().map(|note| {
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(note)
            }))
    }

    fn render_sites(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let structure = self.document.structure();
        let rows = structure.sites.iter().map(|site| {
            let id = site.id;
            let derived = self.document.derived();
            let wyckoff = derived
                .site_symmetry(id)
                .and_then(|s| s.wyckoff.as_ref())
                .map(ToString::to_string);
            let count = derived.multiplicity(id).unwrap_or_default();
            let occupant = site.occupants.first();
            let element = occupant
                .map(|o| o.species.element.clone())
                .unwrap_or_default();
            let occupancy = occupant.map_or(1.0, |o| o.occupancy.value());
            let coordinates = site.position.map(format::coordinate).join(", ");
            let row = match wyckoff {
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
            };
            let row = match &site.disorder {
                Some(disorder) => tr!(
                    "panel.sites.row_disorder",
                    row = row,
                    code = disorder_code(disorder)
                ),
                None => row,
            };
            ListItem::new(("site", id.0 as usize))
                .selected(self.selection.touches_site(id))
                // ⇧- or ⌘-click adds or removes the site, as in the viewport.
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    let modifiers = event.modifiers();
                    if modifiers.shift || modifiers.secondary() {
                        let mut selection = this.selection.clone();
                        selection.toggle_site(id);
                        this.set_selection(selection, window, cx);
                    } else {
                        this.set_selection(Selection::site(id), window, cx);
                    }
                }))
                .child(
                    v_flex()
                        .w_full()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().font_medium().child(site.label().to_string()))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(row),
                                ),
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
                                window.dispatch_action(Box::new(AddSite), cx)
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
