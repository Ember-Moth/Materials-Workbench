//! Everything GPUI draws over the 3D image: the info chip, the hovered
//! atom, the measuring or moving prompt, measurement labels and the
//! floating view toolbar (`docs/design/ui-shell.md` §4.2). Only the toolbar takes the
//! pointer; the chips and labels let clicks through to the atoms.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Selectable, Sizable, StyledExt, h_flex, v_flex,
};
use gpui_kit::*;

use crate::actions::{KEY_CONTEXT, ResetView, ToggleMeasure, ToggleMoveAtoms};
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
        let atom_count = self.document.derived().atoms.len();
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
            Some(self.measure_prompt())
        } else if self.moving {
            Some(self.move_prompt())
        } else {
            None
        };

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
                            .text_color(theme.muted_foreground)
                            .child(tr!("viewport.info", atoms = atom_count)),
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
                // Mock: the popover only explains that display ranges are not
                // implemented yet.
                Popover::new("display-range")
                    .anchor(Anchor::BottomCenter)
                    .trigger(
                        Button::new("display-range-trigger")
                            .ghost()
                            .small()
                            .label(tr!("toolbar.display_range")),
                    )
                    .content(|_, _, cx| {
                        v_flex()
                            .w_64()
                            .gap_1()
                            .text_sm()
                            .child(tr!("toolbar.display_range_title"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(tr!("toolbar.display_range_mock")),
                            )
                    }),
            )
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
