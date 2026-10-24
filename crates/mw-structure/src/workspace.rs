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
use std::sync::Arc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use mw_crystal::{
    AtomRef, CellParam, CellPreset, CellTransform, CellValues, Checkpoint, CoordinateConstraint,
    CrystalDocument, DisplayRange, Edit, EditMode, PolyhedronCenters, SiteId, SphereCenter,
    Structure, element,
};
use mw_crystal_io::ImportedBlock;
use mw_project_file::{
    Camera, DisplayModel as SavedDisplayModel, DisplayRange as SavedDisplayRange, EntryView,
    PanelTab as SavedPanelTab, SphereCenter as SavedSphereCenter, StructureFile,
};
use mw_viewport::{CameraPose, Viewport};

use crate::actions::*;
use crate::cell_dialog::{self, CellDialog};
use crate::display_range::{DisplayRangePanel, DisplayState};
use crate::document::StructureDocument;
use crate::moving::AtomDrag;
use crate::polyhedra::PolyhedraPanel;
use crate::scene::{self, Atoms, DisplayModel, LineColors};
use crate::selection::Selection;
use crate::symmetrize_dialog::{self, SymmetrizeDialog};
use crate::{dialogs, editing, format};

/// Element symbols, the items of every element picker.
pub(crate) type ElementList = SearchableVec<&'static str>;

/// The fields for one occupant of the edited site.
pub(crate) struct OccupantEditor {
    /// The occupant's label in the document.
    pub label: SharedString,
    /// The field that renames it.
    pub label_input: Entity<InputState>,
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

/// A supercell made from the open structure, for the host to add to the
/// project as a derived structure.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Supercell {
    /// The default name, e.g. `金红石 2×2×2 超胞`.
    pub name: SharedString,
    pub structure: Structure,
    /// The structure it was made from, as it was.
    pub source: Arc<Structure>,
    /// Rows of the matrix M of the new lattice `A·M`.
    pub matrix: [[i32; 3]; 3],
}

/// What the workspace asks of the window that hosts it.
#[derive(Clone, Debug)]
pub enum StructureWorkspaceEvent {
    MadeSupercell(Supercell),
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
    /// The centers of the polyhedra in the polyhedra model (crystallography
    /// design §5.6): view state, like the display model.
    pub(crate) polyhedron_centers: PolyhedronCenters,
    pub(crate) polyhedra_panel: Entity<PolyhedraPanel>,
    /// Whether the popover choosing the centers is open.
    pub(crate) polyhedra_open: bool,
    /// What part of the crystal the viewport shows (crystallography design
    /// §5.5): view state, kept apart from the document and its undo.
    pub(crate) display_range: DisplayRange,
    /// The atoms shown for it (`display_range.rs`).
    pub(crate) display: DisplayState,
    pub(crate) range_panel: Entity<DisplayRangePanel>,
    /// Whether the display range popover is open.
    pub(crate) range_open: bool,
    pub(crate) measuring: bool,
    /// Atoms clicked for the measurement in progress.
    pub(crate) measure_picks: Vec<AtomRef>,
    /// Move mode: dragging an atom moves its site (`moving.rs`).
    pub(crate) moving: bool,
    pub(crate) drag: Option<AtomDrag>,
    pub(crate) selection: Selection,
    pub(crate) cell_inputs: Vec<(CellParam, Entity<InputState>)>,
    pub(crate) coordinate_inputs: [Entity<InputState>; 3],
    /// The site the coordinate and occupant fields show.
    pub(crate) editing_site: Option<SiteId>,
    pub(crate) occupants: Vec<OccupantEditor>,
    /// Whether the project has unsaved changes beyond this document's; the
    /// shell knows, since it saves projects.
    project_modified: bool,
    /// The cell transform dialog last opened, while it is open.
    pub(crate) cell_dialog: Option<WeakEntity<CellDialog>>,
    /// The symmetrize dialog last opened, while it is open.
    pub(crate) symmetrize_dialog: Option<WeakEntity<SymmetrizeDialog>>,
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

        let workspace = cx.entity().downgrade();
        let range_panel = cx.new(|cx| DisplayRangePanel::new(workspace.clone(), window, cx));
        let polyhedra_panel = cx.new(|_| PolyhedraPanel::new(workspace));

        let mut this = Self {
            document,
            viewport,
            focus_handle: cx.focus_handle(),
            panel: PanelTab::Properties,
            panel_open: true,
            display_model: DisplayModel::BallAndStick,
            polyhedron_centers: PolyhedronCenters::default(),
            polyhedra_panel,
            polyhedra_open: false,
            display_range: DisplayRange::default(),
            display: DisplayState::default(),
            range_panel,
            range_open: false,
            measuring: false,
            measure_picks: Vec::new(),
            moving: false,
            drag: None,
            selection,
            cell_inputs,
            coordinate_inputs,
            editing_site: None,
            occupants: Vec::new(),
            project_modified: false,
            cell_dialog: None,
            symmetrize_dialog: None,
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

    // MARK: Automation

    /// The domain document, for capabilities that read it (MCP clients
    /// connected to the application). Edits go through [`Self::execute`],
    /// which updates what the workspace shows.
    pub fn crystal(&self) -> &CrystalDocument {
        &self.document.crystal
    }

    /// The data block an imported structure was read from, when its copy
    /// could be read.
    pub fn source(&self) -> Option<&ImportedBlock> {
        self.document.source.as_deref()
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
        let mut view = EntryView::new(
            Some(camera),
            Some(display_model),
            Some(panel),
            Some(self.panel_open),
        );
        // Left out when they are the default, as older views have none.
        if let PolyhedronCenters::Elements(elements) = &self.polyhedron_centers {
            view = view.with_polyhedron_centers(elements.iter().cloned().collect());
        }
        if self.display_range == DisplayRange::default() {
            return view;
        }
        let range = match self.display_range {
            DisplayRange::Cells { counts, boundary } => {
                SavedDisplayRange::Cells { counts, boundary }
            }
            DisplayRange::Sphere { center, radius } => SavedDisplayRange::Sphere {
                center: match center {
                    SphereCenter::Atom(atom) => SavedSphereCenter::Atom(atom),
                    SphereCenter::Point(point) => SavedSphereCenter::Point(point),
                },
                radius,
            },
        };
        view.with_display_range(range)
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
        let centers = view
            .polyhedron_centers
            .as_ref()
            .map_or(PolyhedronCenters::Default, |elements| {
                PolyhedronCenters::Elements(elements.iter().cloned().collect())
            });
        self.set_polyhedron_centers(centers, cx);
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
        if let Some(range) = view.display_range {
            let range = match range {
                SavedDisplayRange::Cells { counts, boundary } => {
                    DisplayRange::Cells { counts, boundary }
                }
                SavedDisplayRange::Sphere { center, radius } => DisplayRange::Sphere {
                    center: match center {
                        SavedSphereCenter::Atom(atom) => SphereCenter::Atom(atom),
                        SavedSphereCenter::Point(point) => SphereCenter::Point(point),
                    },
                    radius,
                },
            };
            self.restore_display_range(range, cx);
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

    fn set_display_model(&mut self, model: DisplayModel, _: &mut Window, cx: &mut Context<Self>) {
        if self.display_model != model {
            self.display_model = model;
            // The centers' popover belongs to the polyhedra.
            self.polyhedra_open &= model == DisplayModel::Polyhedra;
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
    /// occupant fields are made again only when the site or its number of
    /// occupants change, so a field keeps its focus across other edits,
    /// renaming included.
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
        let same_occupants =
            self.editing_site == Some(site.id) && self.occupants.len() == site.occupants.len();
        self.editing_site = Some(site.id);
        if same_occupants {
            for (editor, occupant) in self.occupants.iter_mut().zip(&site.occupants) {
                editor.label = occupant.label.clone().into();
                let label = editor.label.clone();
                editor
                    .label_input
                    .update(cx, |state, cx| state.set_value(label, window, cx));
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
        // The fields name their occupant by position, and read its label when
        // they commit: a rename keeps the fields.
        for (index, occupant) in site.occupants.iter().enumerate() {
            let label = SharedString::from(occupant.label.clone());
            let label_input = cx.new(|cx| InputState::new(window, cx).default_value(label.clone()));
            let element = element_picker(&occupant.species.element, window, cx);
            let occupancy = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(format::occupancy(occupant.occupancy.value()))
            });
            let site = site.id;
            self._occupant_subscriptions.push(cx.subscribe_in(
                &label_input,
                window,
                move |this, _, event, window, cx| {
                    if commits(event) {
                        this.commit_label(index, window, cx);
                    }
                },
            ));
            self._occupant_subscriptions.push(cx.subscribe_in(
                &element,
                window,
                move |this, _, event: &SelectEvent<ElementList>, window, cx| {
                    if let SelectEvent::Confirm(Some(symbol)) = event
                        && let Some(label) = this.occupant_label(index)
                    {
                        this.set_element(site, label, symbol, window, cx);
                    }
                },
            ));
            self._occupant_subscriptions.push(cx.subscribe_in(
                &occupancy,
                window,
                move |this, _, event, window, cx| {
                    if commits(event)
                        && let Some(label) = this.occupant_label(index)
                    {
                        this.commit_occupancy(label, window, cx);
                    }
                },
            ));
            self.occupants.push(OccupantEditor {
                label,
                label_input,
                element,
                occupancy,
            });
        }
    }

    /// The label of the occupant whose fields are at `index`.
    fn occupant_label(&self, index: usize) -> Option<SharedString> {
        self.occupants.get(index).map(|editor| editor.label.clone())
    }

    /// Rebuilds what the viewport draws, after the structure, the display
    /// range, the display model or the measurements change. While the atoms
    /// of a large range are worked out in the background, the viewport
    /// keeps the previous scene, and this runs again when they are ready.
    pub(crate) fn refresh_scene(&mut self, cx: &mut Context<Self>) {
        if !self.update_display(cx) {
            return;
        }
        let colors = self.line_colors(cx);
        let derived = self.document.derived();
        let atoms = Atoms {
            structure: self.document.structure(),
            orbits: &derived.orbits,
            displayed: &self.display.shown,
            ids: None,
        };
        let annotations = self.document.crystal.annotations();
        let scene = scene::draw(&atoms, annotations, self.display_model, colors);
        let reframe = self.take_reframe();
        self.viewport.update(cx, |viewport, cx| {
            viewport.set_scene(scene, cx);
            if reframe {
                viewport.frame_scene(cx);
            }
        });
        self.sync_viewport_selection(cx);
    }

    pub(crate) fn line_colors(&self, cx: &App) -> LineColors {
        let theme = cx.theme();
        LineColors {
            cell: theme.muted_foreground,
            measurement: theme.foreground,
        }
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
        let measuring = !self.measuring;
        if measuring && self.moving {
            self.set_moving(false, cx);
        }
        self.measuring = measuring;
        self.measure_picks.clear();
        if self.measuring {
            self.set_panel(PanelTab::Measurements, cx);
        }
        self.sync_viewport_selection(cx);
        cx.notify();
    }

    /// Esc: puts a dragged atom back, or drops the atoms picked for a
    /// measurement, or else the selection.
    fn clear_selection(&mut self, _: &ClearSelection, window: &mut Window, cx: &mut Context<Self>) {
        if self.cancel_drag(cx) {
            return;
        }
        if self.measuring && !self.measure_picks.is_empty() {
            self.measure_pick(None, window, cx);
        } else {
            self.set_selection(Selection::default(), window, cx);
        }
    }

    fn convert_to_p1(&mut self, _: &ConvertToP1, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        dialogs::open_convert_to_p1(self, entity, window, cx);
    }

    /// Aligns a cell that is a little off its symmetry, as an imported one
    /// may be (crystallography design §7.3): linked parameters take the
    /// value they follow and fixed angles the one the symmetry requires.
    /// One undo step; nothing when the cell keeps its symmetry.
    fn align_cell(&mut self, _: &AlignCell, window: &mut Window, cx: &mut Context<Self>) {
        let edit = Edit::SetCell {
            values: CellValues::default(),
        };
        // A refusal explains itself in a notification.
        let _ = self.edit(edit, window, cx);
    }

    fn open_transform_cell(
        &mut self,
        _: &TransformCell,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entity = cx.entity();
        let dialog = cell_dialog::open(self, entity, window, cx);
        self.cell_dialog = Some(dialog.downgrade());
    }

    fn open_symmetrize(&mut self, _: &Symmetrize, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        let dialog = symmetrize_dialog::open(self, entity, window, cx);
        self.symmetrize_dialog = Some(dialog.downgrade());
    }

    /// Symmetrizes the structure by the symmetry its atoms have
    /// (crystallography design §7.10), as one undo step; the selection and
    /// the measurements follow the atoms. `symprec` is the tolerance in Å,
    /// the structure's own when `None`. `false` when it was refused, which a
    /// notification explains.
    pub fn symmetrize(
        &mut self,
        symprec: Option<f64>,
        standardize: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let edit = Edit::Symmetrize {
            symprec,
            standardize,
        };
        self.edit(edit, window, cx).is_ok()
    }

    /// Describes the structure in another cell (crystallography design
    /// §7.8), as one undo step; the selection and the measurements follow
    /// the atoms. `false` when the change was refused, which a notification
    /// explains.
    pub fn transform_cell(
        &mut self,
        transform: CellTransform,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.edit(Edit::TransformCell { transform }, window, cx)
            .is_ok()
    }

    /// Changes to the cell a preset gives, as choosing it in the dialog and
    /// confirming does. `false` when the structure offers no such change or
    /// it was refused.
    pub fn transform_cell_to(
        &mut self,
        preset: CellPreset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match preset.transform(self.document.structure()) {
            Some(transform) => self.transform_cell(transform, window, cx),
            None => false,
        }
    }

    fn make_supercell(&mut self, _: &MakeSupercell, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        dialogs::open_supercell(self, entity, window, cx);
    }

    /// Makes the supercell repeated `repeats` times along a, b and c in the
    /// background, then asks the host to add it to the project
    /// (design §7.9). This structure does not change.
    pub fn make_supercell_of(
        &mut self,
        repeats: [i32; 3],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let [a, b, c] = repeats;
        let matrix = [[a, 0, 0], [0, b, 0], [0, 0, c]];
        let name = tr!(
            "supercell.name",
            name = self.document.name.to_string(),
            a = a,
            b = b,
            c = c
        );
        let source = self.document.crystal.snapshot();
        let make = cx.background_spawn(async move {
            mw_crystal::make_supercell(&source, matrix).map(|structure| (structure, source))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = make.await;
            this.update_in(cx, |_, window, cx| match result {
                Ok((structure, source)) => {
                    cx.emit(StructureWorkspaceEvent::MadeSupercell(Supercell {
                        name,
                        structure,
                        source,
                        matrix,
                    }))
                }
                Err(error) => window.push_notification(
                    Notification::error(tr!("supercell.failed", reason = error.to_string())),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
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

impl EventEmitter<StructureWorkspaceEvent> for StructureWorkspace {}

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
            // Modifier keys reach the focused workspace, not the viewport
            // inside it: ⌥ during a drag leaves the special position.
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                this.set_drag_leaving(event.modifiers.alt, cx)
            }))
            .on_action(cx.listener(Self::toggle_panel))
            .on_action(cx.listener(Self::reset_view))
            .on_action(cx.listener(Self::toggle_measure))
            .on_action(cx.listener(Self::toggle_move_atoms))
            .on_action(cx.listener(Self::clear_selection))
            .on_action(cx.listener(Self::delete_selection))
            .on_action(cx.listener(Self::add_site))
            .on_action(cx.listener(Self::add_mixed_occupant))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::convert_to_p1))
            .on_action(cx.listener(Self::align_cell))
            .on_action(cx.listener(Self::open_transform_cell))
            .on_action(cx.listener(Self::open_symmetrize))
            .on_action(cx.listener(Self::make_supercell))
            .on_action(cx.listener(Self::export))
            .on_action(cx.listener(Self::show_ball_and_stick))
            .on_action(cx.listener(Self::show_space_filling))
            .on_action(cx.listener(Self::show_polyhedra))
            .on_action(cx.listener(Self::edit_polyhedra))
            .on_action(cx.listener(Self::edit_display_range))
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
