//! Everything GPUI draws over the 3D image: the info chip, the hovered
//! atom or plane point, the measuring, moving or adding prompt, measurement
//! labels and the floating view toolbar with its popovers
//! (`docs/design/ui-shell.md` §4.2). Only the toolbar and the adding
//! prompt's picker take the pointer; the chips and labels let clicks
//! through to the atoms.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Selectable, Sizable, StyledExt, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{KEY_CONTEXT, ResetView, ToggleAddOnPlanes, ToggleMeasure, ToggleMoveAtoms};
use crate::format;
use crate::scene::DisplayModel;
use crate::workspace::StructureWorkspace;

/// Offset from the hovered atom's center to its chip, up and to the right,
/// clear of the pointer. Measured runtime geometry.
const HOVER_OFFSET: Point<Pixels> = Point {
    x: px(14.),
    y: px(-34.),
};

impl StructureWorkspace {
    pub(crate) fn render_viewport_area(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        // Labels give values for the document's positions; while an atom
        // is dragged they would be stale, so they wait for the drop.
        let labels = if self.drag.is_some() {
            Vec::new()
        } else {
            self.measurement_labels(cx)
        };
        let hovered = self.render_hovered_atom(cx);
        let info = match self.ellipsoid_info() {
            Some(note) => tr!(
                "viewport.info_with_note",
                info = self.display_info(),
                note = note
            ),
            None => self.display_info(),
        };
        let preparing = self.is_preparing_display();
        let chip = || {
            div()
                .absolute()
                .px_2()
                .py_0p5()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .text_xs()
        };

        let prompt = if self.measuring {
            Some(self.measure_prompt().into_any_element())
        } else if self.moving {
            Some(self.move_prompt().into_any_element())
        } else if self.adding {
            Some(self.render_add_prompt(cx).into_any_element())
        } else {
            None
        };
        let plane_hover = self.render_plane_hover(cx);

        div()
            .id("structure-viewport")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            // Clicking the 3D view moves keyboard focus into the workspace.
            .on_any_mouse_down(cx.listener(|this, _, window, cx| this.focus(window, cx)))
            .child(self.viewport.clone())
            // The info chip, then the prompt centered in the room beside
            // it, so that a long prompt wraps rather than covers the chip.
            .child(
                h_flex()
                    .absolute()
                    .top_3()
                    .left_3()
                    .right_3()
                    .items_start()
                    .gap_2()
                    .child(
                        chip()
                            .relative()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_color(theme.muted_foreground)
                            .when(preparing, |this| this.child(Spinner::new().xsmall()))
                            .child(info),
                    )
                    .children(prompt.map(|prompt| {
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .justify_center()
                            .child(chip().relative().min_w_0().child(prompt))
                    })),
            )
            .children(hovered)
            .children(plane_hover)
            .children(labels.into_iter().map(|(position, text)| {
                div()
                    .absolute()
                    .left(position.x)
                    .top(position.y)
                    .px_1p5()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .child(text)
            }))
            .child(
                h_flex()
                    .absolute()
                    .bottom_4()
                    .left_0()
                    .right_0()
                    .justify_center()
                    .child(self.render_toolbar(cx)),
            )
    }

    /// The hovered atom's label and fractional coordinates, next to it.
    fn render_hovered_atom(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        let theme = cx.theme();
        let viewport = self.viewport.read(cx);
        let atom = self.picked_atom(viewport.hovered())?;
        let derived = self.document.derived();
        let site = self.document.structure().site(atom.site)?;
        let lattice = &self.document.structure().lattice;
        let position = viewport.project(derived.cartesian(lattice, atom)?)? + HOVER_OFFSET;
        let coordinates = derived.fractional(atom)?.map(format::coordinate).join(", ");
        Some(
            h_flex()
                .absolute()
                .left(position.x)
                .top(position.y)
                .gap_1p5()
                .px_1p5()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .text_xs()
                .child(div().font_medium().child(site.label().to_string()))
                .child(
                    div()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(coordinates),
                ),
        )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let model_button = |id: &'static str, label: SharedString, model: DisplayModel| {
            Button::new(id)
                .ghost()
                .small()
                .selected(self.display_model() == model)
                .label(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.dispatch_display_model(model, window, cx)
                }))
        };
        h_flex()
            .id("view-toolbar")
            // Clicks between the buttons must not select atoms behind.
            .occlude()
            .gap_1()
            .p_1()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .shadow_md()
            .child(model_button(
                "model-ball-stick",
                tr!("toolbar.ball_and_stick"),
                DisplayModel::BallAndStick,
            ))
            .child(model_button(
                "model-space-filling",
                tr!("toolbar.space_filling"),
                DisplayModel::SpaceFilling,
            ))
            .child(model_button(
                "model-polyhedra",
                tr!("toolbar.polyhedra"),
                DisplayModel::Polyhedra,
            ))
            // The centers belong to the polyhedra, so their popover only
            // shows with them.
            .when(self.display_model() == DisplayModel::Polyhedra, |this| {
                let panel = self.polyhedra_panel.clone();
                this.child(
                    Popover::new("polyhedron-centers")
                        .anchor(Anchor::BottomCenter)
                        .open(self.polyhedra_open)
                        .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                            this.set_polyhedra_open(*open, cx)
                        }))
                        .trigger(
                            Button::new("polyhedron-centers-trigger")
                                .ghost()
                                .small()
                                .icon(IconName::ChevronUp)
                                .tooltip(tr!("toolbar.polyhedron_centers"))
                                .accessibility_label(tr!("toolbar.polyhedron_centers")),
                        )
                        .content(move |_, _, _| panel.clone()),
                )
            })
            .child(model_button(
                "model-ellipsoids",
                tr!("toolbar.ellipsoids"),
                DisplayModel::Ellipsoids,
            ))
            // Likewise the probability with the ellipsoids.
            .when(self.display_model() == DisplayModel::Ellipsoids, |this| {
                let panel = self.ellipsoids_panel.clone();
                this.child(
                    Popover::new("ellipsoid-probability")
                        .anchor(Anchor::BottomCenter)
                        .open(self.ellipsoids_open)
                        .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                            this.set_ellipsoids_open(*open, window, cx)
                        }))
                        .trigger(
                            Button::new("ellipsoid-probability-trigger")
                                .ghost()
                                .small()
                                .icon(IconName::ChevronUp)
                                .tooltip(tr!("toolbar.ellipsoid_probability"))
                                .accessibility_label(tr!("toolbar.ellipsoid_probability")),
                        )
                        .content(move |_, _, _| panel.clone()),
                )
            })
            .child(div().w_px().h_5().mx_1().bg(theme.border))
            .child(
                Button::new("toggle-measure")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::Ruler))
                    .label(tr!("toolbar.measure"))
                    .selected(self.measuring)
                    .tooltip_with_action(
                        tr!("toolbar.measure_tooltip"),
                        &ToggleMeasure,
                        Some(KEY_CONTEXT),
                    )
                    .on_click(cx.listener(|_, _, window, cx| {
                        window.dispatch_action(Box::new(ToggleMeasure), cx)
                    })),
            )
            .child(
                Button::new("toggle-move-atoms")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::Move))
                    .label(tr!("toolbar.move"))
                    .selected(self.moving)
                    .tooltip_with_action(
                        tr!("toolbar.move_tooltip"),
                        &ToggleMoveAtoms,
                        Some(KEY_CONTEXT),
                    )
                    .on_click(cx.listener(|_, _, window, cx| {
                        window.dispatch_action(Box::new(ToggleMoveAtoms), cx)
                    })),
            )
            .child(
                Button::new("toggle-add-on-planes")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::MapPinPlus))
                    .label(tr!("toolbar.add"))
                    .selected(self.adding)
                    .tooltip_with_action(
                        tr!("toolbar.add_tooltip"),
                        &ToggleAddOnPlanes,
                        Some(KEY_CONTEXT),
                    )
                    .on_click(cx.listener(|_, _, window, cx| {
                        window.dispatch_action(Box::new(ToggleAddOnPlanes), cx)
                    })),
            )
            .child({
                let panel = self.planes_panel.clone();
                Popover::new("lattice-planes")
                    .anchor(Anchor::BottomCenter)
                    .open(self.planes_open)
                    .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                        this.set_planes_open(*open, window, cx)
                    }))
                    .trigger(
                        Button::new("lattice-planes-trigger")
                            .ghost()
                            .small()
                            .label(tr!("toolbar.planes")),
                    )
                    .content(move |_, _, _| panel.clone())
            })
            .child({
                let panel = self.range_panel.clone();
                Popover::new("display-range")
                    .anchor(Anchor::BottomCenter)
                    .open(self.range_open)
                    .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                        this.set_range_open(*open, window, cx)
                    }))
                    .trigger(
                        Button::new("display-range-trigger")
                            .ghost()
                            .small()
                            .label(tr!("toolbar.display_range")),
                    )
                    .content(move |_, _, _| panel.clone())
            })
            // Only disordered structures have groups to choose from.
            .when(self.has_disorder_groups(), |this| {
                let panel = self.disorder_panel.clone();
                this.child(
                    Popover::new("disorder-groups")
                        .anchor(Anchor::BottomCenter)
                        .open(self.disorder_open)
                        .on_open_change(
                            cx.listener(|this, open: &bool, _, cx| {
                                this.set_disorder_open(*open, cx)
                            }),
                        )
                        .trigger(
                            Button::new("disorder-groups-trigger")
                                .ghost()
                                .small()
                                .label(tr!("toolbar.disorder")),
                        )
                        .content(move |_, _, _| panel.clone()),
                )
            })
            .child(div().w_px().h_5().mx_1().bg(theme.border))
            .child(
                Button::new("reset-view")
                    .ghost()
                    .small()
                    .icon(IconName::Maximize)
                    .tooltip_with_action(tr!("toolbar.reset_view"), &ResetView, Some(KEY_CONTEXT))
                    .on_click(cx.listener(|_, _, window, cx| {
                        window.dispatch_action(Box::new(ResetView), cx)
                    })),
            )
    }
}
