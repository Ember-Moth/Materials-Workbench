//! The "Transform cell…" dialog (crystallography design §7.8): a preset
//! from moyo (the primitive, conventional or Niggli cell, or another
//! tabulated setting) or a typed (P, p). The dialog shows the new cell, the
//! number of atoms in it and its setting as the choice changes; confirming
//! runs one `TransformCell`, which undo reverts.

use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::searchable_list::{
    SearchableGroup, SearchableListDelegate as _, SearchableListItem, SearchableVec,
};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::*;
use mw_crystal::{
    CellParams, CellPreset, CellTransform, Edit, EditError, Rational, Setting, SpaceGroupType,
    Structure, other_settings,
};

use crate::workspace::StructureWorkspace;
use crate::{editing, format, panel};

/// What the choice in the dialog stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    Preset(CellPreset),
    /// The matrix and origin as typed.
    Custom,
}

/// One row of the choice list.
#[derive(Clone)]
pub(crate) struct ChoiceItem {
    choice: Choice,
    title: SharedString,
    /// The structure is in that cell already.
    current: bool,
}

impl SearchableListItem for ChoiceItem {
    type Value = Choice;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &Choice {
        &self.choice
    }

    fn disabled(&self) -> bool {
        self.current
    }
}

type ChoiceList = SearchableVec<SearchableGroup<ChoiceItem>>;

/// What the typed or chosen change of cell gives.
enum Preview {
    Changed {
        params: CellParams,
        atoms: usize,
        setting: SharedString,
    },
    Unchanged,
    /// Why it cannot be applied.
    Refused(SharedString),
}

/// The dialog's state: the choices with their changes of cell, the fields
/// for P (rows a, b, c; columns a′, b′, c′) and p, and the preview of the
/// current values.
pub(crate) struct CellDialog {
    structure: Arc<Structure>,
    atoms: usize,
    presets: Vec<(CellPreset, CellTransform)>,
    pub(crate) choice: Entity<SelectState<ChoiceList>>,
    pub(crate) matrix: [[Entity<InputState>; 3]; 3],
    pub(crate) origin: [Entity<InputState>; 3],
    preview: Preview,
    _subscriptions: Vec<Subscription>,
}

impl CellDialog {
    fn new(
        structure: Arc<Structure>,
        atoms: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let cells = [
            CellPreset::Primitive,
            CellPreset::Conventional,
            CellPreset::Niggli,
        ];
        let settings = other_settings(&structure);
        let mut presets = Vec::new();
        for preset in cells
            .into_iter()
            .chain(settings.iter().map(|s| CellPreset::Setting {
                hall_number: s.hall_number,
            }))
        {
            if let Some(transform) = preset.transform(&structure) {
                presets.push((preset, transform));
            }
        }
        let item = |preset: CellPreset, title: SharedString, current: bool| ChoiceItem {
            choice: Choice::Preset(preset),
            title: if current {
                tr!("dialog.cell.current", name = title.to_string())
            } else {
                title
            },
            current,
        };
        let mut cell_items = Vec::new();
        let mut setting_items = Vec::new();
        for (preset, transform) in &presets {
            let current = transform.is_identity();
            match preset {
                CellPreset::Primitive => {
                    cell_items.push(item(*preset, tr!("dialog.cell.primitive"), current))
                }
                CellPreset::Conventional => {
                    cell_items.push(item(*preset, tr!("dialog.cell.conventional"), current))
                }
                CellPreset::Niggli => {
                    cell_items.push(item(*preset, tr!("dialog.cell.niggli"), current))
                }
                CellPreset::Setting { hall_number } => {
                    let Some(setting) = settings.iter().find(|s| s.hall_number == *hall_number)
                    else {
                        continue;
                    };
                    let title = tr!(
                        "panel.symmetry.setting_value",
                        symbol = format::space_group(setting.hm_symbol),
                        code = setting.code
                    );
                    setting_items.push(item(*preset, title, current));
                }
            }
        }
        cell_items.push(ChoiceItem {
            choice: Choice::Custom,
            title: tr!("dialog.cell.custom"),
            current: false,
        });
        // The first change that does something starts selected.
        let first = cell_items
            .iter()
            .chain(&setting_items)
            .find(|item| !item.current)
            .map_or(Choice::Custom, |item| item.choice);
        let mut groups =
            vec![SearchableGroup::new(tr!("dialog.cell.group_cells")).items(cell_items)];
        if !setting_items.is_empty() {
            groups
                .push(SearchableGroup::new(tr!("dialog.cell.group_settings")).items(setting_items));
        }
        let list = SearchableVec::new(groups);
        let index = list.position(&first);
        let choice = cx.new(|cx| SelectState::new(list, index, window, cx));

        let mut field = |cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx));
        let matrix = [0, 1, 2].map(|_| [field(cx), field(cx), field(cx)]);
        let origin = [field(cx), field(cx), field(cx)];
        let mut subscriptions = vec![cx.subscribe_in(
            &choice,
            window,
            |this, _, event: &SelectEvent<ChoiceList>, window, cx| {
                if let SelectEvent::Confirm(Some(choice)) = event {
                    this.chosen(*choice, window, cx);
                }
            },
        )];
        for input in matrix.iter().flatten().chain(&origin) {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.typed(window, cx);
                    }
                },
            ));
        }
        let mut dialog = Self {
            structure,
            atoms,
            presets,
            choice,
            matrix,
            origin,
            preview: Preview::Unchanged,
            _subscriptions: subscriptions,
        };
        match first {
            Choice::Preset(preset) => dialog.fill(preset, window, cx),
            Choice::Custom => {
                dialog.set_fields(&CellTransform::IDENTITY, window, cx);
                dialog.update_preview(cx);
            }
        }
        dialog
    }

    /// Picks a choice, as the list does: a preset shows its P and p.
    #[cfg(test)]
    pub(crate) fn choose(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        self.choice.update(cx, |state, cx| {
            state.set_selected_value(&choice, window, cx)
        });
        self.chosen(choice, window, cx);
    }

    fn chosen(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        if let Choice::Preset(preset) = choice {
            self.fill(preset, window, cx);
        }
    }

    pub(crate) fn chosen_value(&self, cx: &App) -> Option<Choice> {
        self.choice.read(cx).selected_value().copied()
    }

    /// The atoms the new cell would hold, `None` when the values change
    /// nothing or are refused.
    #[cfg(test)]
    pub(crate) fn previewed_atoms(&self) -> Option<usize> {
        match self.preview {
            Preview::Changed { atoms, .. } => Some(atoms),
            _ => None,
        }
    }

    /// Why the values cannot be applied, if they cannot.
    #[cfg(test)]
    pub(crate) fn refusal(&self) -> Option<SharedString> {
        match &self.preview {
            Preview::Refused(reason) => Some(reason.clone()),
            _ => None,
        }
    }

    fn transform_of(&self, preset: CellPreset) -> Option<CellTransform> {
        self.presets
            .iter()
            .find(|(p, _)| *p == preset)
            .map(|(_, transform)| *transform)
    }

    /// Shows a preset's P and p in the fields.
    fn fill(&mut self, preset: CellPreset, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(transform) = self.transform_of(preset) {
            self.set_fields(&transform, window, cx);
        }
        self.update_preview(cx);
    }

    fn set_fields(&self, transform: &CellTransform, window: &mut Window, cx: &mut Context<Self>) {
        let text = |value: Rational| SharedString::from(value.to_string());
        for (i, row) in self.matrix.iter().enumerate() {
            for (j, input) in row.iter().enumerate() {
                let value = text(transform.matrix()[i][j]);
                input.update(cx, |state, cx| state.set_value(value, window, cx));
            }
        }
        for (i, input) in self.origin.iter().enumerate() {
            let value = text(transform.origin()[i]);
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
    }

    /// A field was edited: the values are now the user's own, unless they
    /// still are the chosen preset's.
    fn typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = self.chosen_value(cx);
        let matches = match (chosen, self.transform(cx)) {
            (Some(Choice::Preset(preset)), Ok(typed)) => self.transform_of(preset) == Some(typed),
            _ => false,
        };
        if !matches && chosen != Some(Choice::Custom) {
            self.choice.update(cx, |state, cx| {
                state.set_selected_value(&Choice::Custom, window, cx)
            });
        }
        self.update_preview(cx);
    }

    /// The change of cell the fields give, or why they give none.
    pub(crate) fn transform(&self, cx: &App) -> Result<CellTransform, SharedString> {
        let read = |input: &Entity<InputState>| {
            let text = input.read(cx).value();
            Rational::parse(&text).ok_or_else(|| {
                if text.trim().is_empty() {
                    tr!("dialog.cell.empty")
                } else {
                    tr!(
                        "dialog.cell.not_a_fraction",
                        value = text.trim().to_string()
                    )
                }
            })
        };
        let mut matrix = [[Rational::ZERO; 3]; 3];
        for (row, inputs) in matrix.iter_mut().zip(&self.matrix) {
            for (value, input) in row.iter_mut().zip(inputs) {
                *value = read(input)?;
            }
        }
        let mut origin = [Rational::ZERO; 3];
        for (value, input) in origin.iter_mut().zip(&self.origin) {
            *value = read(input)?;
        }
        CellTransform::new(matrix, origin)
            .map_err(|error| editing::error_text(&EditError::Transform(error), &self.structure))
    }

    fn update_preview(&mut self, cx: &mut Context<Self>) {
        self.preview = match self.transform(cx) {
            Err(reason) => Preview::Refused(reason),
            Ok(transform) if transform.is_identity() => Preview::Unchanged,
            Ok(transform) => match (Edit::TransformCell { transform }).apply(&self.structure) {
                Err(error) => Preview::Refused(editing::error_text(&error, &self.structure)),
                Ok(applied) => {
                    let structure = applied.structure;
                    let params = *structure.lattice.params();
                    let atoms = mw_crystal::orbits(&structure)
                        .iter()
                        .map(|orbit| orbit.positions.len())
                        .sum();
                    Preview::Changed {
                        params,
                        atoms,
                        setting: setting_text(&structure),
                    }
                }
            },
        };
        cx.notify();
    }
}

/// The space group and setting of a structure, e.g. `R-3c (No. 167),
/// R-3c:R (R)`.
fn setting_text(structure: &Structure) -> SharedString {
    let Some(space_group) =
        SpaceGroupType::identify(&structure.symmetry, structure.lattice.params())
    else {
        return tr!("panel.symmetry.space_group_unknown");
    };
    let name = panel::space_group_name(space_group.number, space_group.hm_symbol);
    let setting = match &space_group.setting {
        Setting::Standard => return name,
        Setting::Tabulated { symbol, code } => tr!(
            "panel.symmetry.setting_value",
            symbol = format::space_group(symbol),
            code = *code
        ),
        Setting::Transformed => tr!("panel.symmetry.setting_transformed"),
    };
    tr!(
        "dialog.cell.space_group_setting",
        space_group = name.to_string(),
        setting = setting.to_string()
    )
}

/// The new cell, lengths on one line and angles on the next.
fn cell_lines(params: &CellParams) -> [SharedString; 2] {
    [
        tr!(
            "dialog.cell.lengths",
            a = format::length(params.a.value()),
            b = format::length(params.b.value()),
            c = format::length(params.c.value())
        ),
        tr!(
            "dialog.cell.angles",
            alpha = format::angle(params.alpha.value()),
            beta = format::angle(params.beta.value()),
            gamma = format::angle(params.gamma.value())
        ),
    ]
}

impl Render for CellDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let heading = |text: SharedString| {
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(muted)
                .child(text)
        };
        // The row labels share one narrow lane, so the four columns of
        // fields line up under their headings.
        let lane = || div().w_6().flex_none().text_xs().text_color(muted);
        let field = |input: &Entity<InputState>, id: String| {
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(input).id(SharedString::from(id)))
        };
        let header = h_flex()
            .gap_2()
            .child(lane())
            .children(["a′", "b′", "c′"].map(|name| heading(name.into())))
            .child(heading(tr!("dialog.cell.origin")));
        let rows = ["a", "b", "c"].iter().enumerate().map(|(i, axis)| {
            h_flex()
                .gap_2()
                .child(lane().child(*axis))
                .children((0..3).map(|j| field(&self.matrix[i][j], format!("cell-p-{i}{j}"))))
                .child(field(&self.origin[i], format!("cell-origin-{i}")))
        });
        // A label beside the first line of its value.
        let row = |label: SharedString, value: AnyElement| {
            h_flex()
                .items_start()
                .gap_3()
                .child(div().w_20().flex_none().text_color(muted).child(label))
                .child(div().flex_1().min_w_0().child(value))
        };
        let preview = match &self.preview {
            Preview::Changed {
                params,
                atoms,
                setting,
            } => v_flex()
                .gap_1()
                .child(row(
                    tr!("dialog.cell.new_cell"),
                    v_flex().children(cell_lines(params)).into_any_element(),
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
                    tr!("panel.symmetry.space_group"),
                    setting.clone().into_any_element(),
                ))
                .into_any_element(),
            Preview::Unchanged => div()
                .text_color(muted)
                .child(tr!("dialog.cell.unchanged"))
                .into_any_element(),
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
                            .child(tr!("dialog.cell.choice")),
                    )
                    .child(
                        Select::new(&self.choice)
                            .id("cell-choice")
                            .menu_max_h(rems(16.)),
                    ),
            )
            .child(v_flex().gap_2().child(header).children(rows))
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
) -> Entity<CellDialog> {
    let structure = workspace.document.crystal.snapshot();
    let atoms = workspace.document.derived().atoms_in_cell();
    let dialog = cx.new(|cx| CellDialog::new(structure, atoms, window, cx));
    let view = dialog.clone();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (view, entity) = (view.clone(), entity.clone());
        alert
            .title(tr!("dialog.cell.title"))
            .description(tr!("dialog.cell.description"))
            .child(view.clone())
            .ok_text(tr!("dialog.cell.ok"))
            .cancel_text(tr!("dialog.cancel"))
            .show_cancel(true)
            .on_ok(move |_, window, cx| match view.read(cx).transform(cx) {
                Err(reason) => {
                    window.push_notification(Notification::error(reason), cx);
                    false
                }
                Ok(transform) => entity.update(cx, |workspace, cx| {
                    workspace.transform_cell(transform, window, cx)
                }),
            })
    });
    dialog
}
