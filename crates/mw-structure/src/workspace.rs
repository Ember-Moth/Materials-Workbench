use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::InputState;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Selectable, Sizable, StyledExt, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal::{CellParam, CoordinateConstraint, EditMode, SiteId};
use mw_viewport::Viewport;

use crate::actions::*;
use crate::document::StructureDocument;
use crate::scene::{self, DisplayModel};
use crate::{dialogs, format};

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
/// panel. Owns presentation state (panel, display model, selection) and the
/// field editors.
///
/// Mock: the field editors accept input, but nothing is applied to the
/// document yet.
pub struct StructureWorkspace {
    pub(crate) document: StructureDocument,
    pub(crate) viewport: Entity<Viewport>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) panel: PanelTab,
    pub(crate) panel_open: bool,
    pub(crate) display_model: DisplayModel,
    pub(crate) measuring: bool,
    pub(crate) selected_site: Option<SiteId>,
    pub(crate) cell_inputs: Vec<(CellParam, Entity<InputState>)>,
    pub(crate) coordinate_inputs: [Entity<InputState>; 3],
    pub(crate) occupancy_input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl StructureWorkspace {
    pub fn new(document: StructureDocument, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let viewport = cx.new(|_| Viewport::new());
        // Overlays (labels) follow the camera, so redraw with the viewport.
        let subscriptions = vec![cx.observe(&viewport, |_, _, cx| cx.notify())];

        let params = *document.structure.lattice.params();
        let cell_inputs = CellParam::ALL
            .iter()
            .map(|&param| {
                let value = match param {
                    CellParam::A => format::length(params.a.value()),
                    CellParam::B => format::length(params.b.value()),
                    CellParam::C => format::length(params.c.value()),
                    CellParam::Alpha => format::angle(params.alpha.value()),
                    CellParam::Beta => format::angle(params.beta.value()),
                    CellParam::Gamma => format::angle(params.gamma.value()),
                };
                (
                    param,
                    cx.new(|cx| InputState::new(window, cx).default_value(value)),
                )
            })
            .collect();
        let mut input = |cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx));
        let coordinate_inputs = [input(cx), input(cx), input(cx)];
        let occupancy_input = input(cx);

        // Start on the first site that has something to edit, which shows the
        // symmetry constraints best.
        let selected_site = document
            .structure
            .sites
            .iter()
            .find(|site| {
                document
                    .derived
                    .site_symmetry(site.id)
                    .is_some_and(|s| s.coordinates.contains(&CoordinateConstraint::Free))
            })
            .or(document.structure.sites.first())
            .map(|site| site.id);

        let mut this = Self {
            document,
            viewport,
            focus_handle: cx.focus_handle(),
            panel: PanelTab::Properties,
            panel_open: true,
            display_model: DisplayModel::BallAndStick,
            measuring: false,
            selected_site,
            cell_inputs,
            coordinate_inputs,
            occupancy_input,
            _subscriptions: subscriptions,
        };
        this.sync_site_inputs(window, cx);
        this.refresh_scene(cx);
        this
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus_handle.focus(window, cx);
    }

    pub(crate) fn select_site(
        &mut self,
        site: SiteId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_site == Some(site) {
            return;
        }
        self.selected_site = Some(site);
        self.sync_site_inputs(window, cx);
        self.refresh_scene(cx);
        cx.notify();
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

    fn sync_site_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(site) = self
            .selected_site
            .and_then(|id| self.document.structure.site(id))
        else {
            return;
        };
        let position = site.position;
        let occupancy = site.occupants.first().map_or(1.0, |o| o.occupancy.value());
        for (input, value) in self.coordinate_inputs.iter().zip(position) {
            input.update(cx, |state, cx| {
                state.set_value(format::coordinate(value), window, cx)
            });
        }
        self.occupancy_input.update(cx, |state, cx| {
            state.set_value(format::occupancy(occupancy), window, cx)
        });
    }

    fn refresh_scene(&mut self, cx: &mut Context<Self>) {
        let spheres = scene::spheres(
            &self.document,
            self.selected_site,
            self.display_model,
            cx.theme().selection,
        );
        self.viewport
            .update(cx, |viewport, cx| viewport.set_scene(&spheres, cx));
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
        if self.measuring {
            self.set_panel(PanelTab::Measurements, cx);
        }
        cx.notify();
    }

    fn delete_selection(
        &mut self,
        _: &DeleteSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        dialogs::open_delete(self, window, cx);
    }

    fn convert_to_p1(&mut self, _: &ConvertToP1, window: &mut Window, cx: &mut Context<Self>) {
        dialogs::open_convert_to_p1(self, window, cx);
    }

    fn make_supercell(&mut self, _: &MakeSupercell, window: &mut Window, cx: &mut Context<Self>) {
        dialogs::open_supercell(self, window, cx);
    }

    fn export(&mut self, _: &ExportStructure, window: &mut Window, cx: &mut Context<Self>) {
        dialogs::not_implemented(tr!("feature.export"), window, cx);
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
        let structure = &self.document.structure;
        match (structure.edit_mode(), self.document.space_group_symbol()) {
            (EditMode::Symmetric, Some(symbol)) => tr!("header.mode.symmetric", symbol = symbol),
            (EditMode::Symmetric, None) => tr!("header.mode.symmetric_unnamed"),
            (EditMode::P1, _) => tr!("header.mode.p1"),
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
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
                    .child(tr!("header.saved")),
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
            .on_action(cx.listener(Self::delete_selection))
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
