//! Add mode (crystallography design §7.6, `docs/design/ui-shell.md` §4.2):
//! a click on a lattice plane adds a site at that point, of the element
//! chosen in the prompt bar. The planes take clicks only in this mode; the
//! viewport gives the point where the line of sight meets the plane
//! clicked, and the site goes through `AddSite` with its snapping, as one
//! undo step. Hovering over a plane tells where the site would go.

use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::Select;
use gpui_kit::component::{ActiveTheme, Sizable, WindowExt, h_flex};
use gpui_kit::*;
use mw_crystal::{Structure, special_position};
use mw_viewport::Pick;

use crate::actions::ToggleAddOnPlanes;
use crate::format;
use crate::scene::ShownPlane;
use crate::workspace::StructureWorkspace;

/// Å. A point this close to its special position counts as on it, as in
/// the add site dialog and the edit commands' reports.
const REPORTED_SNAP: f64 = 1e-4;
/// Offset from the pointer to the chip that tells where a site would go,
/// up and to the right, clear of the crosshair. Measured runtime geometry,
/// as for the hovered atom's chip.
const HOVER_OFFSET: Point<Pixels> = Point {
    x: px(14.),
    y: px(-34.),
};

/// The pointer over a lattice plane in add mode.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlaneHover {
    /// Where it is, in the scene's Cartesian coordinates (Å).
    pub point: [f64; 3],
    /// What adding there would do.
    pub summary: SharedString,
}

/// The fractional coordinates of a point on one of the planes shown: that
/// of the plane it lies on (the nearest, should two meet there), tidied by
/// `LatticePlane::place`.
pub(crate) fn point_on_planes(
    structure: &Structure,
    planes: &[ShownPlane],
    point: [f64; 3],
) -> Option<[f64; 3]> {
    let lattice = &structure.lattice;
    let params = lattice.params();
    let distance = |shown: &ShownPlane| {
        let normal = shown.plane.normal(lattice);
        let along: f64 = (0..3).map(|i| normal[i] * point[i]).sum();
        (along - shown.plane.distance(params)).abs()
    };
    let nearest = planes
        .iter()
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))?;
    Some(nearest.plane.place(lattice.to_fractional(point)))
}

/// Where a site added at `position` goes, as the add site dialog says it:
/// the position, the special position it snaps onto, and its atoms.
fn summary(structure: &Structure, position: [f64; 3]) -> SharedString {
    let special = special_position(structure, position);
    let count = structure.symmetry.operations().len() / special.order.max(1);
    let position = position.map(format::coordinate).join(", ");
    let form = special.form.to_string();
    if special.displacement >= REPORTED_SNAP {
        tr!(
            "add.hover_snaps",
            position = position,
            form = form,
            count = count
        )
    } else {
        tr!("add.hover", position = position, form = form, count = count)
    }
}

impl StructureWorkspace {
    pub(crate) fn toggle_add_on_planes(
        &mut self,
        _: &ToggleAddOnPlanes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_adding(!self.adding, window, cx);
    }

    /// Whether add mode is on.
    pub fn is_adding(&self) -> bool {
        self.adding
    }

    /// Turns add mode on or off. It excludes measuring and moving: each
    /// gives clicks a meaning of its own. Turning it on picks the selected
    /// site's element if none was chosen yet.
    pub fn set_adding(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if on {
            if self.measuring {
                self.measuring = false;
                self.measure_picks.clear();
                self.sync_viewport_selection(cx);
            }
            if self.moving {
                self.set_moving(false, cx);
            }
            self.choose_default_element(window, cx);
        }
        self.set_adding_mode(on, cx);
    }

    /// Turns add mode on or off without touching the other modes.
    pub(crate) fn set_adding_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.adding != on {
            self.adding = on;
            self.plane_hover = None;
            // The planes take clicks only in add mode.
            self.refresh_scene(cx);
            cx.notify();
        }
    }

    /// The first element of the selected site, or of the first site, until
    /// the user chooses another.
    fn choose_default_element(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.add_element.read(cx).selected_value().is_some() {
            return;
        }
        let structure = self.document.structure();
        let site = self
            .selected_site()
            .and_then(|id| structure.site(id))
            .or(structure.sites.first());
        let element = site.map(|site| site.element().to_string());
        let symbol = element.and_then(|e| mw_crystal::element::symbols().find(|s| *s == e));
        if let Some(symbol) = symbol {
            self.add_element.update(cx, |state, cx| {
                state.set_selected_value(&symbol, window, cx)
            });
        }
    }

    /// The element new sites take.
    pub fn add_element(&self, cx: &App) -> Option<&'static str> {
        self.add_element.read(cx).selected_value().copied()
    }

    /// Chooses the element new sites take, as the prompt bar's picker does.
    pub fn set_add_element(
        &mut self,
        element: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_element.update(cx, |state, cx| {
            state.set_selected_value(&element, window, cx)
        });
    }

    /// The pointer moved over a plane, or off it.
    pub(crate) fn hover_plane(
        &mut self,
        pick: Option<Pick>,
        point: Option<[f64; 3]>,
        cx: &mut Context<Self>,
    ) {
        let hover = match (self.adding, pick, point) {
            (true, Some(Pick::Polygon(_)), Some(point)) => {
                let structure = self.document.structure();
                point_on_planes(structure, &self.planes, point).map(|position| PlaneHover {
                    point,
                    summary: summary(structure, position),
                })
            }
            _ => None,
        };
        if self.plane_hover != hover {
            self.plane_hover = hover;
            cx.notify();
        }
    }

    /// A click on a plane in add mode: a site of the chosen element where
    /// it was clicked (design §7.6).
    pub(crate) fn click_plane(
        &mut self,
        point: [f64; 3],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(position) = point_on_planes(self.document.structure(), &self.planes, point) else {
            return;
        };
        let Some(element) = self.add_element(cx) else {
            window.push_notification(Notification::error(tr!("add.no_element")), cx);
            return;
        };
        self.add_site_at(position, element, 1.0, window, cx);
        // The hover told where the site would go; it is there now.
        self.plane_hover = None;
    }

    /// The prompt bar of add mode: the element picker and what to do.
    pub(crate) fn render_add_prompt(&self, cx: &App) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let hint = if self.planes.is_empty() {
            tr!("add.no_planes")
        } else {
            tr!("add.prompt")
        };
        h_flex()
            .id("add-prompt")
            // The picker takes the pointer, unlike the other prompts.
            .occlude()
            .gap_2()
            .min_w_0()
            .child(
                div()
                    .flex_none()
                    .text_color(theme.muted_foreground)
                    .child(tr!("add.element")),
            )
            // The picker fills its parent, which sets its width.
            .child(
                div().w_24().flex_none().child(
                    Select::new(&self.add_element)
                        .id("add-element")
                        .xsmall()
                        .menu_max_h(rems(16.))
                        .placeholder(tr!("dialog.add_occupant.choose_element"))
                        .search_placeholder(tr!("panel.site.search_element")),
                ),
            )
            // One line beside the picker; cut short in a narrow window.
            .child(div().min_w_0().truncate().child(hint))
    }

    /// The chip next to the pointer over a plane in add mode.
    pub(crate) fn render_plane_hover(&self, cx: &App) -> Option<impl IntoElement + use<>> {
        let hover = self.plane_hover.as_ref()?;
        let theme = cx.theme();
        let position = self.viewport.read(cx).project(hover.point)? + HOVER_OFFSET;
        Some(
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
                .whitespace_nowrap()
                .child(hover.summary.clone()),
        )
    }

    /// Where a point of a plane at fractional coordinates appears in the
    /// window, for driving add mode with synthetic pointer events (the
    /// screenshot tour); as for atoms, point once the viewport is current.
    pub fn fractional_point_position(
        &self,
        fractional: [f64; 3],
        cx: &App,
    ) -> Option<Point<Pixels>> {
        let cartesian = self.document.structure().lattice.to_cartesian(fractional);
        let viewport = self.viewport.read(cx);
        Some(viewport.bounds()?.origin + viewport.project(cartesian)?)
    }

    /// What adding at the hovered point would do, while the pointer is on a
    /// plane in add mode.
    #[cfg(test)]
    pub(crate) fn plane_hover_summary(&self) -> Option<SharedString> {
        self.plane_hover.as_ref().map(|hover| hover.summary.clone())
    }
}

#[cfg(test)]
mod tests {
    use mw_crystal::LatticePlane;
    use mw_crystal::sample::{self, SampleKey};

    use super::point_on_planes;
    use crate::scene::ShownPlane;

    fn shown(id: u32, miller: [i32; 3], position: f64) -> ShownPlane {
        ShownPlane {
            id,
            plane: LatticePlane::new(miller, position).unwrap(),
            color: 0,
        }
    }

    /// A point on a plane becomes the plane's fractional coordinates, on
    /// the plane it lies on.
    #[test]
    fn points_go_to_the_plane_they_lie_on() {
        let rutile = sample::load(SampleKey::Rutile).structure;
        let lattice = &rutile.lattice;
        let planes = [shown(1, [0, 0, 1], 0.5), shown(2, [1, 1, 0], 1.0)];
        let on_001 = lattice.to_cartesian([0.31234, 0.25, 0.5000002]);
        assert_eq!(
            point_on_planes(&rutile, &planes, on_001),
            Some([0.3123, 0.25, 0.5])
        );
        let on_110 = lattice.to_cartesian([0.3, 0.7000001, 0.2]);
        let point = point_on_planes(&rutile, &planes, on_110).unwrap();
        assert!((point[0] + point[1] - 1.0).abs() < 1e-12, "{point:?}");
        assert_eq!(point_on_planes(&rutile, &[], on_001), None);
    }
}
