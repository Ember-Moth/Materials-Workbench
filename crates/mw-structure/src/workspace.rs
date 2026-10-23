use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::IndexPath;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, SelectEvent, SelectState};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Selectable, Sizable, StyledExt, Theme, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal::{AtomRef, CellParam, Checkpoint, CoordinateConstraint, EditMode, SiteId, element};
use mw_project_file::{
    Camera, DisplayModel as SavedDisplayModel, EntryView, PanelTab as SavedPanelTab, StructureFile,
};
use mw_viewport::{CameraPose, Viewport};

use crate::actions::*;
use crate::document::StructureDocument;
use crate::scene::{self, DisplayModel, LineColors};
use crate::selection::Selection;
use crate::{dialogs, editing, format};

/// Element symbols, the items of every element picker.
pub(crate) type ElementList = SearchableVec<&'static str>;

/// The fields for one occupant of the edited site.
pub(crate) struct OccupantEditor {
    pub label: SharedString,
    pub element: Entity<SelectState<ElementList>>,
    pub occupancy: Entity<InputState>,
}

/// A searchable element picker showing `selected`.
pub(crate) fn element_picker(
    selected: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<SelectState<ElementList>> {
    let symbols: Vec<&'static str> = element::symbols().collect();
    let index = symbols
        .iter()
        .position(|symbol| *symbol == selected)
        .map(|row| IndexPath::default().row(row));
    cx.new(|cx| SelectState::new(SearchableVec::new(symbols), index, window, cx).searchable(true))
}

/// Whether an input event commits the typed value: Enter, or leaving the
/// field.
fn commits(event: &InputEvent) -> bool {
    matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelTab {
    Properties,
    Sites,
    Measurements,
    Diagnostics,
}

impl PanelTab {
    pub const ALL: [PanelTab; 4] = [
        PanelTab::Properties,
        PanelTab::Sites,
        PanelTab::Measurements,
        PanelTab::Diagnostics,
    ];
}

/// One open structure: header, 3D view with its overlays, and the side
/// panel. Owns presentation state (panel, display model, selection,
/// measurement in progress) and the field editors, which edit the document
/// (`editing.rs`).
pub struct StructureWorkspace {
    pub(crate) document: StructureDocument,
    pub(crate) viewport: Entity<Viewport>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) panel: PanelTab,
    pub(crate) panel_open: bool,
    pub(crate) display_model: DisplayModel,
    pub(crate) measuring: bool,
    /// Atoms clicked for the measurement in progress.
    pub(crate) measure_picks: Vec<AtomRef>,
    pub(crate) selection: Selection,
    pub(crate) cell_inputs: Vec<(CellParam, Entity<InputState>)>,
    pub(crate) coordinate_inputs: [Entity<InputState>; 3],
    /// The site the coordinate and occupant fields show.
    pub(crate) editing_site: Option<SiteId>,
    pub(crate) occupants: Vec<OccupantEditor>,
    /// Whether the project has unsaved changes beyond this document's; the
    /// shell knows, since it saves projects.
    project_modified: bool,
    _subscriptions: Vec<Subscription>,
    _occupant_subscriptions: Vec<Subscription>,
}

impl StructureWorkspace {
    pub fn new(document: StructureDocument, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let viewport = cx.new(|_| Viewport::new());
        let mut subscriptions = vec![
            // Overlays (labels) follow the camera, so redraw with the viewport.
            cx.observe(&viewport, |_, _, cx| cx.notify()),
            cx.subscribe_in(&viewport, window, Self::on_viewport_event),
            // Line colors are part of the scene.
            cx.observe_global::<Theme>(|this, cx| this.refresh_scene(cx)),
        ];

        let params = *document.structure().lattice.params();
        let cell_inputs: Vec<(CellParam, Entity<InputState>)> = CellParam::ALL
            .iter()
            .map(|&param| {
                let value = format::cell_param(param, params.get(param).value());
                (
                    param,
                    cx.new(|cx| InputState::new(window, cx).default_value(value)),
                )
            })
            .collect();
        for (param, input) in &cell_inputs {
            let param = *param;
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this, _, event, window, cx| {
                    if commits(event) {
                        this.commit_cell(param, window, cx);
                    }
                },
            ));
        }
        let mut input = |cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx));
        let coordinate_inputs = [input(cx), input(cx), input(cx)];
        for (axis, input) in coordinate_inputs.iter().enumerate() {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this, _, event, window, cx| {
                    if commits(event) {
                        this.commit_coordinate(axis, window, cx);
                    }
                },
            ));
        }

        // Start on the first site that has something to edit, which shows the
        // symmetry constraints best.
        let selection = document
            .structure()
            .sites
            .iter()
            .find(|site| {
                document
                    .derived()
                    .site_symmetry(site.id)
                    .is_some_and(|s| s.coordinates.contains(&CoordinateConstraint::Free))
            })
            .or(document.structure().sites.first())
            .map_or_else(Selection::default, |site| Selection::site(site.id));

        let mut this = Self {
            document,
            viewport,
            focus_handle: cx.focus_handle(),
            panel: PanelTab::Properties,
            panel_open: true,
            display_model: DisplayModel::BallAndStick,
            measuring: false,
            measure_picks: Vec::new(),
            selection,
            cell_inputs,
            coordinate_inputs,
            editing_site: None,
            occupants: Vec::new(),
            project_modified: false,
            _subscriptions: subscriptions,
            _occupant_subscriptions: Vec::new(),
        };
        this.sync_site_inputs(window, cx);
        this.refresh_scene(cx);
        this
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus_handle.focus(window, cx);
    }

    /// Whether the 3D view shows its newest frame; the screenshot tour
    /// waits for it.
    pub fn viewport_is_current(&self, cx: &App) -> bool {
        self.viewport.read(cx).is_current()
    }

    // MARK: Saving

    /// Whether the structure has edits since it was opened or saved.
    pub fn is_modified(&self) -> bool {
        self.document.is_modified()
    }

    /// The document as the project file holds it, with the state it is in,
    /// to mark that state saved once the file is written.
    pub fn file_to_save(&self) -> (StructureFile, Checkpoint) {
        (self.document.file(), self.document.checkpoint())
    }

    pub fn mark_saved(&mut self, checkpoint: Checkpoint, cx: &mut Context<Self>) {
        self.document.mark_saved(checkpoint);
        cx.notify();
    }

    /// Tells the workspace its project's name, and whether the project has
    /// unsaved changes besides this document's, for the header.
    pub fn set_project(&mut self, name: SharedString, modified: bool, cx: &mut Context<Self>) {
        if self.document.project_name != name || self.project_modified != modified {
            self.document.project_name = name;
            self.project_modified = modified;
            cx.notify();
        }
    }

    /// How the structure is shown, to save with the project
    /// (`docs/design/project-format.md` §8).
    pub fn view(&self, cx: &App) -> EntryView {
        let pose = self.viewport.read(cx).camera_pose();
        let camera = Camera::new(pose.yaw, pose.pitch, pose.half_height, pose.target);
        let display_model = match self.display_model {
            DisplayModel::BallAndStick => SavedDisplayModel::BallAndStick,
            DisplayModel::SpaceFilling => SavedDisplayModel::SpaceFilling,
            DisplayModel::Polyhedra => SavedDisplayModel::Polyhedra,
        };
        let panel = match self.panel {
            PanelTab::Properties => SavedPanelTab::Properties,
            PanelTab::Sites => SavedPanelTab::Sites,
            PanelTab::Measurements => SavedPanelTab::Measurements,
            PanelTab::Diagnostics => SavedPanelTab::Diagnostics,
        };
        EntryView::new(
            Some(camera),
            Some(display_model),
            Some(panel),
            Some(self.panel_open),
        )
    }

    /// Shows the structure as it was when saved.
    pub fn set_view(&mut self, view: &EntryView, cx: &mut Context<Self>) {
        if let Some(model) = view.display_model {
            let model = match model {
                SavedDisplayModel::BallAndStick => DisplayModel::BallAndStick,
                SavedDisplayModel::SpaceFilling => DisplayModel::SpaceFilling,
                SavedDisplayModel::Polyhedra => DisplayModel::Polyhedra,
            };
            if self.display_model != model {
                self.display_model = model;
                self.refresh_scene(cx);
            }
        }
        if let Some(panel) = view.panel {
            self.panel = match panel {
                SavedPanelTab::Properties => PanelTab::Properties,
                SavedPanelTab::Sites => PanelTab::Sites,
                SavedPanelTab::Measurements => PanelTab::Measurements,
                SavedPanelTab::Diagnostics => PanelTab::Diagnostics,
            };
        }
        if let Some(open) = view.panel_open {
            self.panel_open = open;
        }
        // After the scene, so that the zoom is limited to its size.
        if let Some(camera) = view.camera {
            let pose = CameraPose::new(camera.yaw, camera.pitch, camera.half_height, camera.target);
            self.viewport
                .update(cx, |viewport, cx| viewport.set_camera_pose(pose, cx));
        }
        cx.notify();
    }

    pub(crate) fn select_site(
        &mut self,
        site: SiteId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_selection(Selection::site(site), window, cx);
    }

    pub(crate) fn set_panel(&mut self, panel: PanelTab, cx: &mut Context<Self>) {
        self.panel = panel;
        self.panel_open = true;
        cx.notify();
    }

    fn set_display_model(
        &mut self,
        model: DisplayModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if model == DisplayModel::Polyhedra {
            dialogs::not_implemented(tr!("feature.polyhedra"), window, cx);
        }
        if self.display_model != model {
            self.display_model = model;
            self.refresh_scene(cx);
            cx.notify();
        }
    }

    /// Shows the document's cell parameters in the cell fields.
    pub(crate) fn sync_cell_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        let params = *self.document.structure().lattice.params();
        for (param, input) in &self.cell_inputs {
            let value = format::cell_param(*param, params.get(*param).value());
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
    }

    /// Shows the selected site in the coordinate and occupant fields. The
    /// occupant fields are made again only when the site or its occupants'
    /// labels change, so a field keeps its focus across other edits.
    pub(crate) fn sync_site_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(site) = self
            .selected_site()
            .and_then(|id| self.document.structure().site(id))
            .cloned()
        else {
            self.editing_site = None;
            self.occupants.clear();
            self._occupant_subscriptions.clear();
            return;
        };
        for (input, value) in self.coordinate_inputs.iter().zip(site.position) {
            input.update(cx, |state, cx| {
                state.set_value(format::coordinate(value), window, cx)
            });
        }
        let same_occupants = self.editing_site == Some(site.id)
            && self
                .occupants
                .iter()
                .map(|editor| editor.label.as_ref())
                .eq(site.occupants.iter().map(|o| o.label.as_str()));
        self.editing_site = Some(site.id);
        if same_occupants {
            for (editor, occupant) in self.occupants.iter().zip(&site.occupants) {
                let element = occupant.species.element.as_str();
                let symbol = element::symbols().find(|symbol| *symbol == element);
                editor.element.update(cx, |state, cx| match symbol {
                    Some(symbol) => state.set_selected_value(&symbol, window, cx),
                    None => state.set_selected_index(None, window, cx),
                });
                let occupancy = format::occupancy(occupant.occupancy.value());
                editor
                    .occupancy
                    .update(cx, |state, cx| state.set_value(occupancy, window, cx));
            }
            return;
        }
        self.occupants.clear();
        self._occupant_subscriptions.clear();
        for occupant in &site.occupants {
            let label = SharedString::from(occupant.label.clone());
            let element = element_picker(&occupant.species.element, window, cx);
            let occupancy = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(format::occupancy(occupant.occupancy.value()))
            });
            let site = site.id;
            let picked = label.clone();
            self._occupant_subscriptions.push(cx.subscribe_in(
                &element,
                window,
                move |this, _, event: &SelectEvent<ElementList>, window, cx| {
                    if let SelectEvent::Confirm(Some(symbol)) = event {
                        this.set_element(site, picked.clone(), symbol, window, cx);
                    }
                },
            ));
            let typed = label.clone();
            self._occupant_subscriptions.push(cx.subscribe_in(
                &occupancy,
                window,
                move |this, _, event, window, cx| {
                    if commits(event) {
                        this.commit_occupancy(typed.clone(), window, cx);
                    }
                },
            ));
            self.occupants.push(OccupantEditor {
                label,
                element,
                occupancy,
            });
        }
    }

    /// Rebuilds what the viewport draws, after the display model or the
    /// measurements change.
    pub(crate) fn refresh_scene(&mut self, cx: &mut Context<Self>) {
        let theme = cx.theme();
        let colors = LineColors {
            cell: theme.muted_foreground,
            measurement: theme.foreground,
        };
        let scene = scene::scene(&self.document, self.display_model, colors);
        self.viewport
            .update(cx, |viewport, cx| viewport.set_scene(scene, cx));
        self.sync_viewport_selection(cx);
    }

    // MARK: Actions

    fn toggle_panel(&mut self, _: &TogglePanel, _: &mut Window, cx: &mut Context<Self>) {
        self.panel_open = !self.panel_open;
        cx.notify();
    }

    fn reset_view(&mut self, _: &ResetView, _: &mut Window, cx: &mut Context<Self>) {
        self.viewport
            .update(cx, |viewport, cx| viewport.reset_camera(cx));
    }

    fn toggle_measure(&mut self, _: &ToggleMeasure, _: &mut Window, cx: &mut Context<Self>) {
        self.measuring = !self.measuring;
        self.measure_picks.clear();
        if self.measuring {
            self.set_panel(PanelTab::Measurements, cx);
        }
        self.sync_viewport_selection(cx);
        cx.notify();
    }

    /// Esc: drops the atoms picked for a measurement first, then the
    /// selection.
    fn clear_selection(&mut self, _: &ClearSelection, window: &mut Window, cx: &mut Context<Self>) {
        if self.measuring && !self.measure_picks.is_empty() {
            self.measure_pick(None, window, cx);
        } else {
            self.set_selection(Selection::default(), window, cx);
        }
    }

    fn convert_to_p1(&mut self, _: &ConvertToP1, window: &mut Window, cx: &mut Context<Self>) {
        dialogs::open_convert_to_p1(self, window, cx);
    }

    fn make_supercell(&mut self, _: &MakeSupercell, window: &mut Window, cx: &mut Context<Self>) {
        dialogs::open_supercell(self, window, cx);
    }

    fn export(&mut self, _: &ExportStructure, window: &mut Window, cx: &mut Context<Self>) {
        self.open_export(window, cx);
    }

    fn show_ball_and_stick(
        &mut self,
        _: &ShowBallAndStick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_display_model(DisplayModel::BallAndStick, window, cx);
    }

    fn show_space_filling(
        &mut self,
        _: &ShowSpaceFilling,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_display_model(DisplayModel::SpaceFilling, window, cx);
    }

    fn show_polyhedra(&mut self, _: &ShowPolyhedra, window: &mut Window, cx: &mut Context<Self>) {
        self.set_display_model(DisplayModel::Polyhedra, window, cx);
    }

    fn show_properties(&mut self, _: &ShowPropertiesPanel, _: &mut Window, cx: &mut Context<Self>) {
        self.set_panel(PanelTab::Properties, cx);
    }

    fn show_sites(&mut self, _: &ShowSitesPanel, _: &mut Window, cx: &mut Context<Self>) {
        self.set_panel(PanelTab::Sites, cx);
    }

    fn show_measurements(
        &mut self,
        _: &ShowMeasurementsPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_panel(PanelTab::Measurements, cx);
    }

    fn show_diagnostics(
        &mut self,
        _: &ShowDiagnosticsPanel,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_panel(PanelTab::Diagnostics, cx);
    }

    // MARK: Rendering

    pub(crate) fn mode_label(&self) -> SharedString {
        let structure = self.document.structure();
        match (structure.edit_mode(), self.document.space_group_symbol()) {
            (EditMode::Symmetric, Some(symbol)) => tr!("header.mode.symmetric", symbol = symbol),
            (EditMode::Symmetric, None) => tr!("header.mode.symmetric_unnamed"),
            (EditMode::P1, _) => tr!("header.mode.p1"),
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let crystal = &self.document.crystal;
        h_flex()
            .h_12()
            .flex_none()
            .gap_2()
            .pl_4()
            .pr_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_1p5()
                    .min_w_0()
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(self.document.project_name.clone()),
                    )
                    .child(div().text_color(theme.muted_foreground).child("/"))
                    .child(
                        div()
                            .font_semibold()
                            .truncate()
                            .child(self.document.name.clone()),
                    ),
            )
            .child(Tag::secondary().small().child(self.mode_label()))
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    // The project is what ⌘S saves, so its state counts.
                    .child(if self.project_modified || self.document.is_modified() {
                        tr!("header.unsaved")
                    } else {
                        tr!("header.saved")
                    }),
            )
            .child(
                Button::new("undo")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::Undo2))
                    .disabled(crystal.undo_command().is_none())
                    .tooltip_with_action(
                        crystal.undo_command().map_or_else(
                            || tr!("header.undo"),
                            |command| {
                                tr!(
                                    "header.undo_command",
                                    command = editing::command_name(command)
                                )
                            },
                        ),
                        &Undo,
                        Some(KEY_CONTEXT),
                    )
                    .on_click(
                        cx.listener(|_, _, window, cx| window.dispatch_action(Box::new(Undo), cx)),
                    ),
            )
            .child(
                Button::new("redo")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::Redo2))
                    .disabled(crystal.redo_command().is_none())
                    .tooltip_with_action(
                        crystal.redo_command().map_or_else(
                            || tr!("header.redo"),
                            |command| {
                                tr!(
                                    "header.redo_command",
                                    command = editing::command_name(command)
                                )
                            },
                        ),
                        &Redo,
                        Some(KEY_CONTEXT),
                    )
                    .on_click(
                        cx.listener(|_, _, window, cx| window.dispatch_action(Box::new(Redo), cx)),
                    ),
            )
            .child(
                Button::new("export")
                    .outline()
                    .small()
                    .icon(Icon::new(Lucide::Download))
                    .label(tr!("header.export"))
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.export(&ExportStructure, window, cx)
                        }),
                    ),
            )
            .child(
                Button::new("toggle-panel")
                    .ghost()
                    .small()
                    .icon(IconName::PanelRight)
                    .selected(self.panel_open)
                    .tooltip_with_action(
                        tr!("header.toggle_panel"),
                        &TogglePanel,
                        Some(KEY_CONTEXT),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_panel(&TogglePanel, window, cx)
                    })),
            )
    }

    pub(crate) fn display_model(&self) -> DisplayModel {
        self.display_model
    }

    pub(crate) fn dispatch_display_model(
        &mut self,
        model: DisplayModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_display_model(model, window, cx);
    }
}

impl Focusable for StructureWorkspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for StructureWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("structure-workspace")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(Self::toggle_panel))
            .on_action(cx.listener(Self::reset_view))
            .on_action(cx.listener(Self::toggle_measure))
            .on_action(cx.listener(Self::clear_selection))
            .on_action(cx.listener(Self::delete_selection))
            .on_action(cx.listener(Self::add_mixed_occupant))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::convert_to_p1))
            .on_action(cx.listener(Self::make_supercell))
            .on_action(cx.listener(Self::export))
            .on_action(cx.listener(Self::show_ball_and_stick))
            .on_action(cx.listener(Self::show_space_filling))
            .on_action(cx.listener(Self::show_polyhedra))
            .on_action(cx.listener(Self::show_properties))
            .on_action(cx.listener(Self::show_sites))
            .on_action(cx.listener(Self::show_measurements))
            .on_action(cx.listener(Self::show_diagnostics))
            .child(self.render_header(cx))
            .child(
                h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_viewport_area(window, cx))
                    .when(self.panel_open, |this| {
                        this.child(self.render_panel(window, cx))
                    }),
            )
    }
}
