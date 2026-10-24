//! Coordination polyhedra in the "polyhedra" display model (crystallography
//! design §5.6, `docs/design/ui-shell.md` §5): which elements are centers,
//! chosen in a popover beside the model's toolbar button. Like the display
//! model, the choice is view state: not undone, not an unsaved change, and
//! saved with the view.

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{ActiveTheme, Sizable, StyledExt, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal::{CenterElement, PolyhedronCenters, Structure};

use crate::actions::EditPolyhedra;
use crate::scene::DisplayModel;
use crate::workspace::StructureWorkspace;

impl StructureWorkspace {
    /// The centers of the polyhedra drawn: `None` outside the polyhedra
    /// model.
    pub(crate) fn polyhedra_shown(&self) -> Option<PolyhedronCenters> {
        (self.display_model == DisplayModel::Polyhedra).then(|| self.polyhedron_centers.clone())
    }

    /// Which atoms are the centers of polyhedra in the polyhedra model.
    pub fn polyhedron_centers(&self) -> &PolyhedronCenters {
        &self.polyhedron_centers
    }

    /// Chooses the centers of the polyhedra.
    pub fn set_polyhedron_centers(&mut self, centers: PolyhedronCenters, cx: &mut Context<Self>) {
        if self.polyhedron_centers != centers {
            self.polyhedron_centers = centers;
            self.refresh_scene(cx);
            cx.notify();
        }
    }

    /// Turns the polyhedra of one element's atoms on or off. The default
    /// choice becomes the user's own from then on.
    pub(crate) fn set_center_element(&mut self, element: &str, on: bool, cx: &mut Context<Self>) {
        let mut elements = self.center_set();
        if on {
            elements.insert(element.to_string());
        } else {
            elements.remove(element);
        }
        self.set_polyhedron_centers(PolyhedronCenters::Elements(elements), cx);
    }

    /// The elements whose atoms are centers now.
    fn center_set(&self) -> BTreeSet<String> {
        let structure = self.document.structure();
        self.document
            .derived()
            .coordination(structure)
            .centers(&self.polyhedron_centers)
    }

    /// Opens or closes the popover that chooses the centers.
    pub fn set_polyhedra_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.polyhedra_open != open {
            self.polyhedra_open = open;
            cx.notify();
        }
    }

    /// The command palette's "Polyhedra…": shows the polyhedra and opens
    /// the popover beside their button.
    pub(crate) fn edit_polyhedra(
        &mut self,
        _: &EditPolyhedra,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dispatch_display_model(DisplayModel::Polyhedra, window, cx);
        self.set_polyhedra_open(true, cx);
    }
}

/// The popover's content: a checkbox for each element whose atoms can be
/// centers, with their numbers of ligands. Changes take effect at once.
pub(crate) struct PolyhedraPanel {
    workspace: WeakEntity<StructureWorkspace>,
    /// The elements listed, and the structure they were found in.
    elements: Option<(Arc<Structure>, Vec<CenterElement>)>,
}

impl PolyhedraPanel {
    pub(crate) fn new(workspace: WeakEntity<StructureWorkspace>) -> Self {
        Self {
            workspace,
            elements: None,
        }
    }

    /// The elements that can be centers in the workspace's structure, found
    /// again only when the structure changed.
    fn elements(&mut self, workspace: &StructureWorkspace) -> &[CenterElement] {
        let structure = workspace.document.crystal.snapshot();
        let stale = self
            .elements
            .as_ref()
            .is_none_or(|(found, _)| !Arc::ptr_eq(found, &structure));
        if stale {
            let elements = workspace
                .document
                .derived()
                .coordination(&structure)
                .elements();
            self.elements = Some((structure, elements));
        }
        self.elements.as_ref().map_or(&[], |(_, e)| e)
    }
}

/// "6", or "4, 6" for an element whose atoms differ.
fn coordination_numbers(numbers: &[usize]) -> String {
    let separator = tr!("polyhedra.separator");
    numbers
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(&separator)
}

impl Render for PolyhedraPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let Some(workspace) = self.workspace.upgrade() else {
            return div().into_any_element();
        };
        let (elements, chosen) = {
            let workspace = workspace.read(cx);
            (self.elements(workspace).to_vec(), workspace.center_set())
        };
        let label = |text: SharedString| div().text_xs().text_color(muted).child(text);

        v_flex()
            .id("polyhedra-panel")
            .w_64()
            .gap_3()
            .text_sm()
            .child(div().font_medium().child(tr!("polyhedra.title")))
            .map(|this| {
                if elements.is_empty() {
                    return this.child(label(tr!("polyhedra.none")));
                }
                this.child(
                    v_flex()
                        .gap_2()
                        .child(label(tr!("polyhedra.centers")))
                        .children(elements.into_iter().map(|element| {
                            let symbol = element.element.clone();
                            let numbers = coordination_numbers(&element.coordination);
                            let hint = if element.coordination.len() == 1 {
                                tr!("polyhedra.coordination", numbers = numbers)
                            } else {
                                tr!("polyhedra.coordinations", numbers = numbers)
                            };
                            let workspace = workspace.downgrade();
                            Checkbox::new(SharedString::from(format!("polyhedra-center-{symbol}")))
                                .small()
                                .label(symbol.clone())
                                .checked(chosen.contains(&symbol))
                                .child(label(hint))
                                .on_click(move |checked: &bool, _, cx| {
                                    let checked = *checked;
                                    workspace
                                        .update(cx, |workspace, cx| {
                                            workspace.set_center_element(&symbol, checked, cx)
                                        })
                                        .ok();
                                })
                        })),
                )
            })
            .into_any_element()
    }
}
