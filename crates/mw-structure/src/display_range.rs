//! The display range (crystallography design §5.5, `docs/design/ui-shell.md`
//! §5): which part of the crystal the viewport shows, a block of cells or a
//! sphere around an atom or a point. The workspace keeps the atoms shown and
//! remakes them when the structure or the range changes, in the background
//! for large ranges; the toolbar's popover sets the range. It is view
//! state: not undone, not an unsaved change.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{
    Input, InputEvent, InputState, NumberInput, NumberInputEvent, StepAction,
};
use gpui_kit::component::{
    ActiveTheme, Disableable, Selectable, Sizable, StyledExt, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal::{
    AtomRef, DisplayRange, DisplayRangeError, Displayed, MAX_DISPLAY_ATOMS, PolyhedronCenters,
    SphereCenter, Structure,
};

use crate::actions::EditDisplayRange;
use crate::format;
use crate::selection::Selected;
use crate::workspace::StructureWorkspace;

/// Atoms above which the atoms shown are worked out in the background, so
/// that a large range does not hold up the interface (10 × 10 × 10 rutile
/// cells are 6531 atoms).
pub(crate) const BACKGROUND_ATOMS: usize = 5_000;
/// Å, the radius a sphere starts with.
const DEFAULT_RADIUS: f64 = 6.0;

/// What the atoms shown are made for: a structure, a range, and in the
/// polyhedra model the centers of the polyhedra.
#[derive(Clone)]
pub(crate) struct DisplayKey {
    structure: Arc<Structure>,
    range: DisplayRange,
    polyhedra: Option<PolyhedronCenters>,
}

impl DisplayKey {
    fn is(&self, other: &DisplayKey) -> bool {
        Arc::ptr_eq(&self.structure, &other.structure)
            && self.range == other.range
            && self.polyhedra == other.polyhedra
    }

    /// The atoms, bonds and polyhedra shown.
    fn display(&self, orbits: &[mw_crystal::Orbit], bonds: &[mw_crystal::Bond]) -> Displayed {
        let structure = &self.structure;
        match &self.polyhedra {
            Some(centers) => {
                Displayed::with_polyhedra(structure, orbits, bonds, &self.range, centers)
            }
            None => Displayed::new(structure, orbits, bonds, &self.range),
        }
    }
}

/// What the workspace shows of the crystal, and what it is working out.
#[derive(Default)]
pub(crate) struct DisplayState {
    /// What the scene draws: sphere i is atom i.
    pub shown: Displayed,
    /// What `shown` was made for.
    made_for: Option<DisplayKey>,
    /// What is being worked out in the background.
    pub preparing: Option<DisplayKey>,
    /// The background work; replacing it drops work no longer wanted.
    _job: Option<Task<()>>,
    /// Whether the view frames the next scene, made for a new range.
    reframe: bool,
}

impl DisplayState {
    fn is_for(&self, key: &DisplayKey) -> bool {
        self.made_for.as_ref().is_some_and(|made| made.is(key))
    }

    fn is_preparing(&self, key: &DisplayKey) -> bool {
        self.preparing.as_ref().is_some_and(|made| made.is(key))
    }
}

/// `n` with its digits grouped by thousands, e.g. `162 000`.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut text = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            // A narrow no-break space, as SI writes large numbers.
            text.push('\u{202F}');
        }
        text.push(digit);
    }
    text
}

/// Why a range cannot be shown, in the interface language.
fn refusal(error: &DisplayRangeError) -> SharedString {
    match error {
        DisplayRangeError::NoCells => tr!("display_range.error.cells"),
        DisplayRangeError::InvalidRadius => tr!("display_range.error.radius"),
        DisplayRangeError::UnknownCenter(_) => tr!("display_range.error.center"),
        DisplayRangeError::TooManyAtoms { atoms } => tr!(
            "display_range.error.too_many",
            atoms = grouped(*atoms),
            limit = grouped(MAX_DISPLAY_ATOMS)
        ),
    }
}

impl StructureWorkspace {
    /// What part of the crystal the viewport shows.
    pub fn display_range(&self) -> DisplayRange {
        self.display_range
    }

    /// Shows another part of the crystal and frames the view on it, or says
    /// why the range cannot be shown, leaving the range as it was.
    pub fn set_display_range(
        &mut self,
        range: DisplayRange,
        cx: &mut Context<Self>,
    ) -> Result<(), DisplayRangeError> {
        range.check(self.document.structure(), &self.document.derived().orbits)?;
        self.show_range(range, cx);
        Ok(())
    }

    fn show_range(&mut self, range: DisplayRange, cx: &mut Context<Self>) {
        if self.display_range != range {
            self.display_range = range;
            self.display.reframe = true;
            self.refresh_scene(cx);
            cx.notify();
        }
    }

    /// Whether atoms for the current structure and range are still being
    /// worked out, while the viewport shows the previous ones.
    pub fn is_preparing_display(&self) -> bool {
        self.display.preparing.is_some()
    }

    /// The range a saved view asks for, if it can be shown: the saved one
    /// comes from another session, maybe of an earlier structure.
    pub(crate) fn restore_display_range(&mut self, range: DisplayRange, cx: &mut Context<Self>) {
        if self.set_display_range(range, cx).is_err() {
            // Shown whatever the structure holds, as before ranges existed.
            self.show_range(DisplayRange::default(), cx);
        }
        // A restored view keeps its saved camera.
        self.display.reframe = false;
    }

    /// The center a sphere's coordinates refer to before an edit, for
    /// keeping the sphere in place once its atom is gone.
    pub(crate) fn display_center(&self) -> Option<[f64; 3]> {
        self.display_range.center(&self.document.derived().orbits)
    }

    /// Brings the atoms shown up to date with the structure, the range and
    /// the polyhedra. `false` while they are worked out in the background:
    /// the scene then waits for them.
    pub(crate) fn update_display(&mut self, cx: &mut Context<Self>) -> bool {
        let structure = self.document.crystal.snapshot();
        let range = self.display_range;
        let key = DisplayKey {
            structure: structure.clone(),
            range,
            polyhedra: self.polyhedra_shown(),
        };
        if self.display.is_for(&key) {
            return true;
        }
        if self.display.is_preparing(&key) {
            return false;
        }
        let derived = self.document.derived();
        let atoms = match range.check(&structure, &derived.orbits) {
            Ok(atoms) => atoms,
            // The range was within the limit when it was set; an edit such
            // as a smaller cell can put it over, and it is shown anyway.
            Err(DisplayRangeError::TooManyAtoms { atoms }) => atoms,
            // Nothing else changes with the structure: a sphere's atom is
            // followed through every edit (`DisplayRange::remap`). Should one
            // still be lost, the default range shows the structure.
            Err(_) => {
                self.display_range = DisplayRange::default();
                return self.update_display(cx);
            }
        };
        if atoms <= BACKGROUND_ATOMS {
            self.display.shown = key.display(&derived.orbits, &derived.bonds);
            self.display.made_for = Some(key);
            self.display.preparing = None;
            self.display._job = None;
            return true;
        }
        let (orbits, bonds) = (derived.orbits.clone(), derived.bonds.clone());
        let job = key.clone();
        let work = cx.background_spawn(async move { job.display(&orbits, &bonds) });
        self.display.preparing = Some(key);
        self.display._job = Some(cx.spawn(async move |this, cx| {
            let shown = work.await;
            this.update(cx, |this, cx| {
                this.display.shown = shown;
                this.display.made_for = this.display.preparing.take();
                this.refresh_scene(cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
        false
    }

    /// Whether the view frames the scene being set, which was made for a
    /// new range.
    pub(crate) fn take_reframe(&mut self) -> bool {
        std::mem::take(&mut self.display.reframe)
    }

    /// The atom a sphere would be centered on: the selected atom added
    /// last, or the first atom of a selected site in the cell at the origin.
    fn selected_center(&self) -> Option<AtomRef> {
        match self.selection.last()? {
            Selected::Atom(atom) => Some(atom),
            Selected::Site(site) => Some(AtomRef::new(site, 0, [0; 3])),
        }
    }

    /// The info chip's text: what is shown, or what is being worked out.
    pub(crate) fn display_info(&self) -> SharedString {
        let atoms = self.display.shown.atoms.len();
        match (&self.display.preparing, self.display_range) {
            (
                Some(_),
                DisplayRange::Cells {
                    counts: [a, b, c], ..
                },
            ) => {
                tr!("viewport.preparing_cells", a = a, b = b, c = c)
            }
            (Some(_), DisplayRange::Sphere { radius, .. }) => {
                tr!("viewport.preparing_sphere", radius = format::radius(radius))
            }
            (
                None,
                DisplayRange::Cells {
                    counts: [a, b, c], ..
                },
            ) => {
                tr!("viewport.info_cells", atoms = atoms, a = a, b = b, c = c)
            }
            (None, DisplayRange::Sphere { radius, .. }) => tr!(
                "viewport.info_sphere",
                atoms = atoms,
                radius = format::radius(radius)
            ),
        }
    }

    pub(crate) fn edit_display_range(
        &mut self,
        _: &EditDisplayRange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_range_open(true, window, cx);
    }

    /// Opens or closes the display range popover; opening shows the
    /// current range in its fields.
    pub fn set_range_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.range_open = open;
        self.sync_range_panel(window, cx);
        cx.notify();
    }

    /// Shows the range in the popover's fields while it is open, after the
    /// range changed or an edit moved its center.
    pub(crate) fn sync_range_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.range_open {
            let view = RangeView::of(self);
            self.range_panel
                .update(cx, |panel, cx| panel.show(view, window, cx));
        }
    }
}

/// What the popover shows of the workspace's range.
struct RangeView {
    range: DisplayRange,
    /// The sphere's center in fractional coordinates.
    center: Option<[f64; 3]>,
    /// The site of the atom it is centered on.
    center_site: Option<String>,
    can_center_on_selection: bool,
}

impl RangeView {
    fn of(workspace: &StructureWorkspace) -> Self {
        let range = workspace.display_range;
        let center_site = match range {
            DisplayRange::Sphere {
                center: SphereCenter::Atom(atom),
                ..
            } => workspace
                .document
                .structure()
                .site(atom.site)
                .map(|site| site.label().to_string()),
            _ => None,
        };
        Self {
            range,
            center: workspace.display_center(),
            center_site,
            can_center_on_selection: workspace.selected_center().is_some(),
        }
    }
}

/// The popover's fields. Changes take effect at once: the segments, the
/// steppers and the checkbox when used, typed values on Enter or when the
/// field loses focus.
pub(crate) struct DisplayRangePanel {
    workspace: WeakEntity<StructureWorkspace>,
    /// The last block of cells and sphere shown, to switch back to.
    cells: ([u32; 3], bool),
    sphere: Option<(SphereCenter, f64)>,
    pub(crate) counts: [Entity<InputState>; 3],
    pub(crate) center: [Entity<InputState>; 3],
    pub(crate) radius: Entity<InputState>,
    /// What the range shown is centered on, for the line under the center.
    center_label: Option<SharedString>,
    can_center_on_selection: bool,
    /// Why the values typed last cannot be shown.
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl DisplayRangePanel {
    pub(crate) fn new(
        workspace: WeakEntity<StructureWorkspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut input = |cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx));
        let counts = [input(cx), input(cx), input(cx)];
        let center = [input(cx), input(cx), input(cx)];
        let radius = input(cx);
        let mut subscriptions = Vec::new();
        for (axis, state) in counts.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if commits(event) {
                        this.commit_cells(window, cx);
                    }
                },
            ));
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                move |this, _, event: &NumberInputEvent, window, cx| {
                    let NumberInputEvent::Step(step) = event;
                    this.step_count(axis, *step, window, cx);
                },
            ));
        }
        for state in center.iter().chain([&radius]) {
            subscriptions.push(cx.subscribe_in(
                state,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if commits(event) {
                        this.commit_sphere(window, cx);
                    }
                },
            ));
        }
        Self {
            workspace,
            cells: ([1; 3], true),
            sphere: None,
            counts,
            center,
            radius,
            center_label: None,
            can_center_on_selection: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    /// Shows the workspace's range in the fields, and forgets what was
    /// typed and refused.
    fn show(&mut self, view: RangeView, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        self.can_center_on_selection = view.can_center_on_selection;
        match view.range {
            DisplayRange::Cells { counts, boundary } => self.cells = (counts, boundary),
            DisplayRange::Sphere { center, radius } => self.sphere = Some((center, radius)),
        }
        let (counts, _) = self.cells;
        for (state, count) in self.counts.iter().zip(counts) {
            state.update(cx, |state, cx| {
                state.set_value(count.to_string(), window, cx)
            });
        }
        if let DisplayRange::Sphere { center, radius } = view.range {
            for (state, value) in self.center.iter().zip(view.center.unwrap_or_default()) {
                let text = format::coordinate(value);
                state.update(cx, |state, cx| state.set_value(text, window, cx));
            }
            self.center_label = Some(match (center, view.center_site) {
                (SphereCenter::Atom(_), Some(label)) => {
                    tr!("display_range.center_atom", label = label)
                }
                _ => tr!("display_range.center_point"),
            });
            let text = format::radius(radius);
            self.radius
                .update(cx, |state, cx| state.set_value(text, window, cx));
        }
        cx.notify();
    }

    /// Shows `range`, or why it cannot be shown.
    fn apply(&mut self, range: DisplayRange, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let result = workspace.update(cx, |workspace, cx| workspace.set_display_range(range, cx));
        match result {
            Ok(()) => {
                let view = RangeView::of(workspace.read(cx));
                self.show(view, window, cx);
            }
            Err(error) => {
                self.error = Some(refusal(&error));
                cx.notify();
            }
        }
    }

    pub(crate) fn show_cells(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (counts, boundary) = self.cells;
        self.apply(DisplayRange::Cells { counts, boundary }, window, cx);
    }

    /// A sphere as last shown, or around the selected atom, else the
    /// cell's center.
    pub(crate) fn show_sphere(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (center, radius) = self.sphere.unwrap_or_else(|| {
            let atom = self
                .workspace
                .upgrade()
                .and_then(|workspace| workspace.read(cx).selected_center());
            let center = atom.map_or(SphereCenter::Point([0.5; 3]), SphereCenter::Atom);
            (center, DEFAULT_RADIUS)
        });
        self.apply(DisplayRange::Sphere { center, radius }, window, cx);
    }

    fn typed_counts(&self, cx: &App) -> Option<[u32; 3]> {
        let mut counts = [0; 3];
        for (count, state) in counts.iter_mut().zip(&self.counts) {
            *count = state.read(cx).value().trim().parse::<u32>().ok()?;
        }
        Some(counts)
    }

    fn commit_cells(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(counts) = self.typed_counts(cx) else {
            self.error = Some(refusal(&DisplayRangeError::NoCells));
            cx.notify();
            return;
        };
        if counts == self.cells.0 && self.error.is_none() {
            return;
        }
        let boundary = self.cells.1;
        self.apply(DisplayRange::Cells { counts, boundary }, window, cx);
    }

    /// A stepper's button or arrow key: one cell more or fewer, at least
    /// one.
    fn step_count(
        &mut self,
        axis: usize,
        step: StepAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut counts = self.typed_counts(cx).unwrap_or(self.cells.0);
        counts[axis] = match step {
            StepAction::Increment => counts[axis].saturating_add(1),
            StepAction::Decrement => counts[axis].saturating_sub(1).max(1),
        };
        let text = counts[axis].to_string();
        self.counts[axis].update(cx, |state, cx| state.set_value(text, window, cx));
        let boundary = self.cells.1;
        self.apply(DisplayRange::Cells { counts, boundary }, window, cx);
    }

    pub(crate) fn set_boundary(
        &mut self,
        boundary: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let counts = self.typed_counts(cx).unwrap_or(self.cells.0);
        self.apply(DisplayRange::Cells { counts, boundary }, window, cx);
    }

    /// The typed center and radius. Coordinates that still read as the
    /// center shown keep it, so that leaving a field around an atom does
    /// not turn it into a point.
    fn commit_sphere(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((center, _)) = self.sphere else {
            return;
        };
        let number = |state: &Entity<InputState>| state.read(cx).value().trim().parse::<f64>();
        let radius = number(&self.radius).unwrap_or(f64::NAN);
        let Ok(typed) = self
            .center
            .iter()
            .map(number)
            .collect::<Result<Vec<f64>, _>>()
        else {
            self.error = Some(tr!("display_range.error.coordinates"));
            cx.notify();
            return;
        };
        let typed = [typed[0], typed[1], typed[2]];
        let shown = self.workspace.upgrade().and_then(|workspace| {
            let orbits = &workspace.read(cx).document.derived().orbits;
            DisplayRange::Sphere { center, radius }.center(orbits)
        });
        let unchanged = shown.is_some_and(|shown| {
            (0..3).all(|i| format::coordinate(shown[i]) == format::coordinate(typed[i]))
        });
        let center = if unchanged {
            center
        } else {
            SphereCenter::Point(typed)
        };
        self.apply(DisplayRange::Sphere { center, radius }, window, cx);
    }

    /// Why the values typed last cannot be shown.
    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }

    pub(crate) fn center_on_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let atom = self
            .workspace
            .upgrade()
            .and_then(|workspace| workspace.read(cx).selected_center());
        if let Some(atom) = atom {
            let radius = self.sphere.map_or(DEFAULT_RADIUS, |(_, radius)| radius);
            let range = DisplayRange::Sphere {
                center: SphereCenter::Atom(atom),
                radius,
            };
            self.apply(range, window, cx);
        }
    }
}

/// Whether an input event commits the typed value: Enter, or leaving the
/// field.
fn commits(event: &InputEvent) -> bool {
    matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur)
}

impl Render for DisplayRangePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let range = self
            .workspace
            .upgrade()
            .map_or_else(DisplayRange::default, |workspace| {
                workspace.read(cx).display_range
            });
        let is_sphere = matches!(range, DisplayRange::Sphere { .. });
        let label = |text: SharedString| div().text_xs().text_color(muted).child(text);
        let prefix = |text: &'static str| div().text_color(muted).child(text);

        let kinds = ButtonGroup::new("display-range-kind")
            .outline()
            .small()
            .child(
                Button::new("display-range-cells")
                    .flex_1()
                    .label(tr!("display_range.kind.cells"))
                    .selected(!is_sphere),
            )
            .child(
                Button::new("display-range-sphere")
                    .flex_1()
                    .label(tr!("display_range.kind.sphere"))
                    .selected(is_sphere),
            )
            .on_click(cx.listener(
                |this, clicks: &Vec<usize>, window, cx| match clicks.first() {
                    Some(0) => this.show_cells(window, cx),
                    Some(1) => this.show_sphere(window, cx),
                    _ => {}
                },
            ));

        let cells = || {
            let (_, boundary) = self.cells;
            v_flex()
                .gap_3()
                .child(
                    v_flex()
                        .gap_1()
                        .child(label(tr!("display_range.counts")))
                        .child(
                            h_flex()
                                .gap_2()
                                .children(self.counts.iter().zip(["a", "b", "c"]).map(
                                    |(state, axis)| {
                                        NumberInput::new(state)
                                            .small()
                                            .flex_1()
                                            .min_w_0()
                                            .prefix(prefix(axis))
                                            .accessibility_label(tr!(
                                                "display_range.count_label",
                                                axis = axis
                                            ))
                                    },
                                )),
                        ),
                )
                .child(
                    Checkbox::new("display-range-boundary")
                        .small()
                        .label(tr!("display_range.boundary"))
                        .checked(boundary)
                        .child(label(tr!("display_range.boundary_hint")))
                        .on_click(cx.listener(|this, checked: &bool, window, cx| {
                            this.set_boundary(*checked, window, cx)
                        })),
                )
        };

        let sphere = || {
            v_flex()
                .gap_3()
                .child(
                    v_flex()
                        .gap_1()
                        .child(label(tr!("display_range.center")))
                        .child(
                            h_flex()
                                .gap_2()
                                .children(self.center.iter().zip(["x", "y", "z"]).map(
                                    |(state, axis)| {
                                        div().flex_1().min_w_0().child(
                                            Input::new(state)
                                                .small()
                                                .prefix(prefix(axis))
                                                .aria_label(tr!(
                                                    "display_range.coordinate_label",
                                                    axis = axis
                                                )),
                                        )
                                    },
                                )),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_between()
                                .children(self.center_label.clone().map(label))
                                .child(
                                    Button::new("display-range-center-selection")
                                        .outline()
                                        .xsmall()
                                        .label(tr!("display_range.center_on_selection"))
                                        .disabled(!self.can_center_on_selection)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.center_on_selection(window, cx)
                                        })),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .child(label(tr!("display_range.radius")))
                        .child(
                            div().w_32().child(
                                Input::new(&self.radius)
                                    .small()
                                    .suffix(prefix("Å"))
                                    .aria_label(tr!("display_range.radius")),
                            ),
                        ),
                )
        };

        v_flex()
            .id("display-range-panel")
            .w_80()
            .gap_3()
            .text_sm()
            .child(div().font_medium().child(tr!("display_range.title")))
            .child(kinds)
            .map(|this| {
                if is_sphere {
                    this.child(sphere())
                } else {
                    this.child(cells())
                }
            })
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_xs().text_color(theme.danger).child(error)),
            )
    }
}
