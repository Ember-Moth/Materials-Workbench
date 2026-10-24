//! Moving atoms by dragging them in the 3D view (crystallography design
//! §7.4, `docs/design/ui-shell.md` §4.2). In move mode a press on an atom
//! drags its site with all equivalent atoms: the viewport shows where they
//! would go, and releasing the button makes it one `MoveSite` step. The
//! site stays on its special position unless ⌥ (Alt) is held; landing near
//! a special position snaps onto it. A press elsewhere still turns the
//! view, and Esc puts the atom back.

use gpui_kit::*;
use mw_crystal::{AtomFate, AtomRef, Edit, Lattice};
use mw_viewport::{Pick, Scene};

use crate::actions::ToggleMoveAtoms;
use crate::display_range::BACKGROUND_ATOMS;
use crate::format;
use crate::scene::{self, Preview};
use crate::selection::Selection;
use crate::workspace::StructureWorkspace;

/// An atom being dragged.
pub(crate) struct AtomDrag {
    /// The atom pressed on, in the document as it is.
    pub atom: AtomRef,
    /// Å, how far the pointer has carried it, in the plane of the screen.
    offset: [f64; 3],
    /// Whether the site may leave its special position: ⌥ is held.
    leave: bool,
    /// The structure if the atom were dropped now.
    preview: Option<Preview>,
    /// A preview being worked out in the background, for a large range.
    job: Option<Task<()>>,
    /// Whether the drag changed since that preview was started.
    moved: bool,
}

#[cfg(test)]
impl AtomDrag {
    /// Where the preview shown puts a site.
    pub(crate) fn previewed_position(&self, site: mw_crystal::SiteId) -> Option<[f64; 3]> {
        Some(self.preview.as_ref()?.structure.site(site)?.position)
    }

    /// How many polyhedra the preview shown draws.
    pub(crate) fn previewed_polyhedra(&self) -> Option<usize> {
        Some(self.preview.as_ref()?.displayed.polyhedra.len())
    }
}

/// The key that leaves the special position, as the platform names it.
pub(crate) fn leave_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌥"
    } else {
        "Alt"
    }
}

/// Fractional coordinates of a Cartesian displacement in Å, through the
/// lattice whatever its orientation: Cramer's rule on the cell vectors.
fn to_fractional(lattice: &Lattice, r: [f64; 3]) -> [f64; 3] {
    let [a, b, c] =
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|axis| lattice.to_cartesian(axis));
    // det[u v w] = u · (v × w).
    let det = |u: [f64; 3], v: [f64; 3], w: [f64; 3]| {
        u[0] * (v[1] * w[2] - v[2] * w[1]) - u[1] * (v[0] * w[2] - v[2] * w[0])
            + u[2] * (v[0] * w[1] - v[1] * w[0])
    };
    let volume = det(a, b, c);
    [
        det(r, b, c) / volume,
        det(a, r, c) / volume,
        det(a, b, r) / volume,
    ]
}

impl StructureWorkspace {
    pub(crate) fn toggle_move_atoms(
        &mut self,
        _: &ToggleMoveAtoms,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_moving(!self.moving, cx);
    }

    /// Turns move mode on or off. It and measuring exclude each other: each
    /// gives clicks and drags on atoms a meaning of its own.
    pub(crate) fn set_moving(&mut self, on: bool, cx: &mut Context<Self>) {
        if on && self.measuring {
            self.measuring = false;
            self.measure_picks.clear();
            self.sync_viewport_selection(cx);
        }
        if !on {
            self.cancel_drag(cx);
        }
        self.moving = on;
        self.viewport
            .update(cx, |viewport, cx| viewport.set_sphere_dragging(on, cx));
        cx.notify();
    }

    /// The pointer carried the pressed atom `offset` Å from where it was.
    pub(crate) fn drag_atom(
        &mut self,
        sphere: usize,
        offset: [f64; 3],
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        // The spheres on screen are not yet those of the structure while a
        // large range is worked out.
        if !self.moving || self.is_preparing_display() {
            return;
        }
        if self.drag.is_none() {
            let Some(atom) = self.picked_atom(Some(Pick::Sphere(sphere))) else {
                return;
            };
            self.drag = Some(AtomDrag {
                atom,
                offset,
                leave: false,
                preview: None,
                job: None,
                moved: false,
            });
        }
        if let Some(drag) = &mut self.drag {
            drag.offset = offset;
            drag.leave = modifiers.alt;
        }
        self.preview_drag(cx);
    }

    /// ⌥ went down or up: the preview follows without the pointer moving.
    pub(crate) fn set_drag_leaving(&mut self, leave: bool, cx: &mut Context<Self>) {
        if let Some(drag) = &mut self.drag
            && drag.leave != leave
        {
            drag.leave = leave;
            self.preview_drag(cx);
        }
    }

    fn move_edit(&self, drag: &AtomDrag) -> Edit {
        let lattice = &self.document.structure().lattice;
        Edit::MoveSite {
            atom: drag.atom,
            displacement: to_fractional(lattice, drag.offset),
            leave_special_position: drag.leave,
        }
    }

    /// Shows the structure as dropping the atom now would leave it, with
    /// every atom of the dragged site ringed, since they all move. For a
    /// large range the preview is worked out in the background, one at a
    /// time: moves made meanwhile are previewed together once it is shown.
    fn preview_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = &self.drag else {
            return;
        };
        if drag.job.is_some() {
            if let Some(drag) = &mut self.drag {
                drag.moved = true;
            }
            return;
        }
        let structure = self.document.crystal.snapshot();
        let edit = self.move_edit(drag);
        let site = drag.atom.site;
        // The region shown stays put, even around the atom being dragged.
        let range = self.display_range.fixed(&self.document.derived().orbits);
        let annotations = self.document.crystal.annotations().to_vec();
        let (model, colors) = (self.display_model, self.line_colors(cx));
        let polyhedra = self.polyhedra_shown();
        let make = move || {
            let applied = edit.apply(&structure).ok()?;
            let preview = Preview::new(applied.structure, applied.ids, &range, polyhedra.as_ref());
            let scene = scene::draw(&preview.atoms(), &annotations, model, colors);
            let rings: Vec<usize> = preview
                .displayed
                .atoms
                .iter()
                .enumerate()
                .filter_map(|(ix, atom)| (atom.atom.site == site).then_some(ix))
                .collect();
            Some((preview, scene, rings))
        };
        if self.display.shown.atoms.len() <= BACKGROUND_ATOMS {
            self.show_preview(make(), cx);
            return;
        }
        let work = cx.background_spawn(async move { make() });
        let job = cx.spawn(async move |this, cx| {
            let made = work.await;
            this.update(cx, |this, cx| {
                let Some(drag) = &mut this.drag else {
                    return;
                };
                drag.job = None;
                let moved = std::mem::take(&mut drag.moved);
                this.show_preview(made, cx);
                if moved {
                    this.preview_drag(cx);
                }
            })
            .ok();
        });
        if let Some(drag) = &mut self.drag {
            drag.job = Some(job);
        }
    }

    /// Shows a preview made for the drag; `None` when the move would be
    /// refused, which leaves the last one shown.
    fn show_preview(&mut self, made: Option<(Preview, Scene, Vec<usize>)>, cx: &mut Context<Self>) {
        let preview = made.map(|(preview, scene, rings)| {
            self.viewport.update(cx, |viewport, cx| {
                viewport.preview_scene(scene, cx);
                viewport.set_selection(rings, cx);
            });
            preview
        });
        if let Some(drag) = &mut self.drag {
            drag.preview = preview;
        }
        cx.notify();
    }

    /// The button went up: the drag becomes one step, and the moved atom
    /// the selection, so the properties panel shows its site.
    pub(crate) fn drop_atom(
        &mut self,
        sphere: usize,
        offset: [f64; 3],
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A drag decided only when the button went up has not been seen.
        if self.drag.is_none() {
            self.drag_atom(sphere, offset, modifiers, cx);
        }
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        drag.offset = offset;
        drag.leave = modifiers.alt;
        let edit = self.move_edit(&drag);
        match self.edit(edit, window, cx) {
            Ok(outcome) if outcome.changed => {
                let selection = match outcome.ids.atom(drag.atom) {
                    AtomFate::Kept(atom) => Selection::atom(atom),
                    AtomFate::Site(site) => Selection::site(site),
                    AtomFate::Removed => Selection::default(),
                };
                self.set_selection(selection, window, cx);
            }
            // Nothing moved, or the move was refused: the preview goes.
            _ => {
                self.refresh_scene(cx);
                cx.notify();
            }
        }
    }

    /// Esc during a drag puts the atom back; `false` when nothing was being
    /// dragged.
    pub(crate) fn cancel_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let dragging = self.viewport.read(cx).is_dragging_sphere();
        if self.drag.take().is_none() && !dragging {
            return false;
        }
        self.viewport
            .update(cx, |viewport, cx| viewport.cancel_sphere_drag(cx));
        self.refresh_scene(cx);
        cx.notify();
        true
    }

    /// The line shown over the viewport in move mode: how to drag, then
    /// where the dragged site goes and whether its multiplicity changes.
    pub(crate) fn move_prompt(&self) -> SharedString {
        let dragged = self
            .drag
            .as_ref()
            .and_then(|drag| Some((drag.atom.site, drag.preview.as_ref()?)));
        let Some((site, preview)) = dragged else {
            return tr!("move.prompt", key = leave_key());
        };
        let label = self
            .document
            .structure()
            .site(site)
            .map_or_else(String::new, |s| s.label().to_string());
        let position = preview.structure.site(site).map_or_else(String::new, |s| {
            s.position.map(format::coordinate).join(", ")
        });
        let before = self.document.derived().multiplicity(site).unwrap_or(0);
        let after = preview
            .orbits
            .iter()
            .find(|orbit| orbit.site == site)
            .map_or(0, |orbit| orbit.positions.len());
        if before == after {
            tr!("move.dragging", label = label, position = position)
        } else {
            tr!(
                "move.dragging_multiplicity",
                label = label,
                position = position,
                before = before,
                after = after
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use mw_crystal::{CellParams, Lattice};

    #[test]
    fn displacements_become_fractional() {
        let lattice = Lattice::new(CellParams::new(3.0, 3.0, 5.0, 90.0, 90.0, 120.0));
        for x in [[0.1, -0.2, 0.3], [1.0, 0.0, 0.0], [0.0, 0.0, -2.0]] {
            let r = lattice.to_cartesian(x);
            let back = super::to_fractional(&lattice, r);
            for i in 0..3 {
                assert!((back[i] - x[i]).abs() < 1e-12, "{back:?} != {x:?}");
            }
        }
    }
}
