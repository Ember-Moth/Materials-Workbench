//! Everything GPUI draws over the 3D image: the info chip, measurement
//! labels and the floating view toolbar (`docs/design/ui-shell.md` §4.2).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Selectable, Sizable, h_flex, v_flex};
use gpui_kit::*;

use crate::actions::{KEY_CONTEXT, ResetView, ToggleMeasure};
use crate::scene::DisplayModel;
use crate::workspace::StructureWorkspace;

/// Offset from a measurement's midpoint to its label's top-left corner, so
/// the label sits just above the bond. Measured runtime geometry.
const LABEL_OFFSET: Point<Pixels> = Point {
    x: px(-32.),
    y: px(-30.),
};

impl StructureWorkspace {
    pub(crate) fn render_viewport_area(
        &self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let viewport = self.viewport.read(cx);
        let labels: Vec<_> = self
            .document
            .measurements
            .iter()
            .filter_map(|m| {
                let position = viewport.project(m.midpoint())?;
                Some((position + LABEL_OFFSET, format!("{:.3} Å", m.distance())))
            })
            .collect();
        let atom_count = self.document.derived.atoms.len();

        div()
            .id("structure-viewport")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            // Clicking the 3D view moves keyboard focus into the workspace.
            .on_any_mouse_down(cx.listener(|this, _, window, cx| this.focus(window, cx)))
            .child(self.viewport.clone())
            .child(
                div()
                    .absolute()
                    .top_3()
                    .left_3()
                    .px_2()
                    .py_0p5()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(tr!("viewport.info", atoms = atom_count)),
            )
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
