//! Lattice planes (crystallography design §5.7, `docs/design/ui-shell.md`
//! §5): the planes (hkl) drawn over the structure, each cut to the display
//! range in a color of its own. The toolbar's popover lists them, moves
//! them and adds new ones. Like the display range, they are view state:
//! not undone, not an unsaved change, and saved with the view.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, IconName, Sizable, StyledExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal::{LatticePlane, PlaneError};

use crate::actions::EditPlanes;
use crate::display_range::commits;
use crate::format;
use crate::scene::{PLANE_COLORS, ShownPlane};
use crate::workspace::StructureWorkspace;

/// The position a new plane takes: one interplanar spacing from the
/// origin, where textbooks draw (100), (110) and (111).
const NEW_POSITION: f64 = 1.0;

impl StructureWorkspace {
    /// The lattice planes drawn, in the order they were added, as their
    /// Miller indices and positions.
    pub fn planes(&self) -> Vec<LatticePlane> {
        self.planes.iter().map(|shown| shown.plane).collect()
    }

    /// Draws another lattice plane, in the first color no plane has.
    pub fn add_plane(&mut self, plane: LatticePlane, window: &mut Window, cx: &mut Context<Self>) {
        self.push_plane(plane, cx);
        self.sync_planes_panel(window, cx);
    }

    /// Adds a plane without showing it in the popover, which asked for it.
    fn push_plane(&mut self, plane: LatticePlane, cx: &mut Context<Self>) {
        let color = (0..PLANE_COLORS.len())
            .find(|color| self.planes.iter().all(|shown| shown.color != *color))
            .unwrap_or(self.planes.len() % PLANE_COLORS.len());
        self.next_plane_id += 1;
        self.planes.push(ShownPlane {
            id: self.next_plane_id,
            plane,
            color,
        });
        self.planes_changed(cx);
    }

    /// Replaces the planes drawn, as a saved view has them: Miller indices,
    /// position and color.
    /// The popover, which lists the planes it was opened with, closes; it
    /// shows these when it opens again.
    pub(crate) fn restore_planes(
        &mut self,
        planes: impl IntoIterator<Item = (LatticePlane, usize)>,
        cx: &mut Context<Self>,
    ) {
        self.planes_open = false;
        self.planes.clear();
        for (plane, color) in planes {
            self.next_plane_id += 1;
            self.planes.push(ShownPlane {
                id: self.next_plane_id,
                plane,
                color: color % PLANE_COLORS.len(),
            });
        }
        self.refresh_scene(cx);
        cx.notify();
    }

    /// Removes a plane, as its button in the popover does.
    #[cfg(test)]
    pub(crate) fn remove_plane(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_plane(id, cx);
        self.sync_planes_panel(window, cx);
    }

    fn drop_plane(&mut self, id: u32, cx: &mut Context<Self>) {
        self.planes.retain(|shown| shown.id != id);
        self.planes_changed(cx);
    }

    /// Moves a plane to another position, in interplanar spacings. The
    /// popover, which asks for it, shows the result itself.
    fn set_plane_position(
        &mut self,
        id: u32,
        position: f64,
        cx: &mut Context<Self>,
    ) -> Result<(), PlaneError> {
        let Some(shown) = self.planes.iter_mut().find(|shown| shown.id == id) else {
            return Ok(());
        };
        let moved = shown.plane.with_position(position)?;
        if moved != shown.plane {
            shown.plane = moved;
            self.planes_changed(cx);
        }
        Ok(())
    }

    fn planes_changed(&mut self, cx: &mut Context<Self>) {
        self.refresh_scene(cx);
        cx.notify();
    }

    /// Opens or closes the planes popover; opening shows the planes drawn
    /// in its fields.
    pub fn set_planes_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.planes_open = open;
        self.sync_planes_panel(window, cx);
        cx.notify();
    }

    /// Shows the planes in the popover's fields while it is open.
    fn sync_planes_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.planes_open {
            let planes = self.planes.clone();
            self.planes_panel
                .update(cx, |panel, cx| panel.show(&planes, window, cx));
        }
    }

    pub(crate) fn edit_planes(
        &mut self,
        _: &EditPlanes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_planes_open(true, window, cx);
    }
}

/// "(1 −1 0)": Miller indices with a true minus sign.
pub(crate) fn miller_label(miller: [i32; 3]) -> String {
    let index = |i: i32| {
        if i < 0 {
            format!("\u{2212}{}", i.unsigned_abs())
        } else {
            i.to_string()
        }
    };
    format!(
        "({} {} {})",
        index(miller[0]),
        index(miller[1]),
        index(miller[2])
    )
}

/// A plane's color as the interface paints its swatch: the data color the
/// viewport draws it in.
fn swatch_color(color: usize) -> Rgba {
    let [r, g, b] = PLANE_COLORS[color % PLANE_COLORS.len()];
    Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
}

/// The popover's content: the planes drawn, each with its spacing and a
/// field for its position, and a row of h, k, l fields to add one. Changes
/// take effect at once: typed values on Enter or when the field loses
/// focus.
pub(crate) struct PlanesPanel {
    workspace: WeakEntity<StructureWorkspace>,
    pub(crate) miller: [Entity<InputState>; 3],
    /// The position field of each plane listed, by the plane's id.
    pub(crate) positions: Vec<(u32, Entity<InputState>)>,
    /// Why the values typed last cannot be used.
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
    _position_subscriptions: Vec<Subscription>,
}

impl PlanesPanel {
    pub(crate) fn new(
        workspace: WeakEntity<StructureWorkspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut input = |cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx));
        let miller = [input(cx), input(cx), input(cx)];
        let subscriptions = miller
            .iter()
            .map(|state| {
                cx.subscribe_in(state, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.add(window, cx);
                    }
                })
            })
            .collect();
        Self {
            workspace,
            miller,
            positions: Vec::new(),
            error: None,
            _subscriptions: subscriptions,
            _position_subscriptions: Vec::new(),
        }
    }

    /// Shows the planes in the fields, keeping the fields of planes still
    /// listed, and forgets what was refused.
    fn show(&mut self, planes: &[ShownPlane], window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        let mut kept = std::mem::take(&mut self.positions);
        let mut subscriptions = Vec::new();
        for shown in planes {
            let state = match kept.iter().position(|(id, _)| *id == shown.id) {
                Some(index) => kept.swap_remove(index).1,
                None => cx.new(|cx| InputState::new(window, cx)),
            };
            let text = format::coordinate(shown.plane.position());
            state.update(cx, |state, cx| state.set_value(text, window, cx));
            let id = shown.id;
            subscriptions.push(cx.subscribe_in(
                &state,
                window,
                move |this, _, event: &InputEvent, window, cx| {
                    if commits(event) {
                        this.commit_position(id, window, cx);
                    }
                },
            ));
            self.positions.push((id, state));
        }
        self._position_subscriptions = subscriptions;
        cx.notify();
    }

    /// The typed Miller indices: whole numbers, not all zero.
    fn typed_miller(&self, cx: &App) -> Result<[i32; 3], SharedString> {
        let mut miller = [0; 3];
        for (index, state) in miller.iter_mut().zip(&self.miller) {
            let text = state.read(cx).value();
            // A minus sign as typography writes it, too.
            let text = text.trim().replace('\u{2212}', "-");
            *index = text
                .parse::<i32>()
                .map_err(|_| tr!("planes.error.indices"))?;
        }
        if miller == [0; 3] {
            return Err(tr!("planes.error.zero"));
        }
        Ok(miller)
    }

    /// Enter in an h, k or l field, or the add button: draws the plane
    /// one spacing from the origin and empties the fields for the next.
    pub(crate) fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let plane = self.typed_miller(cx).and_then(|miller| {
            LatticePlane::new(miller, NEW_POSITION).map_err(|_| tr!("planes.error.zero"))
        });
        let plane = match plane {
            Ok(plane) => plane,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        for state in &self.miller {
            state.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.change(window, cx, |workspace, cx| {
            workspace.push_plane(plane, cx);
            Ok(())
        });
    }

    /// Changes the workspace's planes, then shows them, or why the change
    /// was refused.
    fn change(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(
            &mut StructureWorkspace,
            &mut Context<StructureWorkspace>,
        ) -> Result<(), PlaneError>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let result = workspace.update(cx, change);
        let planes = workspace.read(cx).planes.clone();
        self.show(&planes, window, cx);
        if result.is_err() {
            self.error = Some(tr!("planes.error.position"));
            cx.notify();
        }
    }

    fn commit_position(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, state)) = self.positions.iter().find(|(plane, _)| *plane == id) else {
            return;
        };
        let text = state.read(cx).value();
        let Ok(position) = text.trim().replace('\u{2212}', "-").parse::<f64>() else {
            self.error = Some(tr!("planes.error.position"));
            cx.notify();
            return;
        };
        self.change(window, cx, |workspace, cx| {
            workspace.set_plane_position(id, position, cx)
        });
    }

    fn remove(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.change(window, cx, |workspace, cx| {
            workspace.drop_plane(id, cx);
            Ok(())
        });
    }

    /// Why the values typed last cannot be used.
    #[cfg(test)]
    pub(crate) fn error(&self) -> Option<SharedString> {
        self.error.clone()
    }
}

impl Render for PlanesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let label = |text: SharedString| div().text_xs().text_color(muted).child(text);
        let prefix = |text: &'static str| div().text_color(muted).child(text);

        // Each plane with its spacing and whether the range shows it.
        let rows: Vec<(ShownPlane, f64, bool)> = self
            .workspace
            .upgrade()
            .map(|workspace| {
                let workspace = workspace.read(cx);
                let structure = workspace.document.structure();
                let orbits = &workspace.document.derived().orbits;
                let range = workspace.display_range();
                workspace
                    .planes
                    .iter()
                    .map(|shown| {
                        let lattice = &structure.lattice;
                        let spacing = shown.plane.spacing(lattice.params());
                        let visible = shown.plane.section(lattice, &range, orbits).is_some();
                        (*shown, spacing, visible)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let list = rows.into_iter().filter_map(|(shown, spacing, visible)| {
            let state = self
                .positions
                .iter()
                .find(|(id, _)| *id == shown.id)
                .map(|(_, state)| state.clone())?;
            let name = miller_label(shown.plane.miller());
            let detail = if visible {
                tr!("planes.spacing", d = format::length(spacing))
            } else {
                tr!("planes.spacing_outside", d = format::length(spacing))
            };
            let id = shown.id;
            Some(
                h_flex()
                    .id(("plane", id))
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .size_3()
                            .rounded_sm()
                            .bg(swatch_color(shown.color)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().font_family(mono.clone()).child(name.clone()))
                            .child(label(detail)),
                    )
                    .child(
                        div().w_20().flex_none().child(
                            Input::new(&state)
                                .small()
                                .suffix(prefix("d"))
                                .aria_label(tr!("planes.position_label", plane = name.clone())),
                        ),
                    )
                    .child(
                        Button::new(("remove-plane", id))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip(tr!("planes.remove"))
                            .accessibility_label(tr!("planes.remove_label", plane = name))
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.remove(id, window, cx)),
                            ),
                    ),
            )
        });
        let list: Vec<_> = list.collect();

        v_flex()
            .id("planes-panel")
            .w_80()
            .gap_3()
            .text_sm()
            .child(div().font_medium().child(tr!("planes.title")))
            .map(|this| {
                if list.is_empty() {
                    this.child(label(tr!("planes.empty")))
                } else {
                    this.child(v_flex().gap_2().children(list))
                        .child(label(tr!("planes.position_hint")))
                }
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(label(tr!("planes.add_title")))
                    .child(
                        h_flex()
                            .gap_2()
                            .children(self.miller.iter().zip(["h", "k", "l"]).map(
                                |(state, index)| {
                                    div().flex_1().min_w_0().child(
                                        Input::new(state)
                                            .small()
                                            .prefix(prefix(index))
                                            .aria_label(tr!("planes.index_label", index = index)),
                                    )
                                },
                            ))
                            .child(
                                Button::new("add-plane")
                                    .outline()
                                    .small()
                                    .label(tr!("planes.add"))
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.add(window, cx)),
                                    ),
                            ),
                    ),
            )
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_xs().text_color(theme.danger).child(error)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::miller_label;

    #[test]
    fn miller_labels_use_minus_signs() {
        assert_eq!(miller_label([1, -1, 0]), "(1 \u{2212}1 0)");
        assert_eq!(miller_label([0, 0, 12]), "(0 0 12)");
    }
}
