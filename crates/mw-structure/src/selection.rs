//! The workspace's one selection, shared by the site list and the viewport
//! (`docs/design/ui-shell.md` §4.2–§4.3): the list selects whole sites, the
//! viewport single atoms; ⇧- or ⌘-click adds and removes atoms.

use gpui_kit::*;
use mw_crystal::{AtomFate, AtomRef, IdMap, SiteId, Target};
use mw_viewport::{Pick, Viewport, ViewportEvent};

use crate::workspace::StructureWorkspace;

/// One selected item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Selected {
    /// Every equivalent atom of a site.
    Site(SiteId),
    Atom(AtomRef),
}

impl Selected {
    fn site(self) -> SiteId {
        match self {
            Self::Site(site) => site,
            Self::Atom(atom) => atom.site,
        }
    }
}

/// Selected sites and atoms, in the order they were added.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Selection(Vec<Selected>);

impl Selection {
    pub fn site(site: SiteId) -> Self {
        Self(vec![Selected::Site(site)])
    }

    pub fn atom(atom: AtomRef) -> Self {
        Self(vec![Selected::Atom(atom)])
    }

    /// The site the properties panel shows: that of the item added last.
    pub fn primary_site(&self) -> Option<SiteId> {
        self.0.last().map(|item| item.site())
    }

    /// Whether the site or any of its atoms is selected.
    pub fn touches_site(&self, site: SiteId) -> bool {
        self.0.iter().any(|item| item.site() == site)
    }

    /// Whether the atom is selected, by itself or as part of its site.
    pub fn contains_atom(&self, atom: AtomRef) -> bool {
        self.0
            .iter()
            .any(|item| *item == Selected::Atom(atom) || *item == Selected::Site(atom.site))
    }

    /// What an edit of the selection names: whole sites, and single atoms.
    pub fn targets(&self) -> Vec<Target> {
        self.0
            .iter()
            .map(|item| match *item {
                Selected::Site(site) => Target::Site(site),
                Selected::Atom(atom) => Target::Atom(atom),
            })
            .collect()
    }

    /// The sites selected or with a selected atom, in selection order.
    pub fn sites(&self) -> Vec<SiteId> {
        let mut sites = Vec::new();
        for item in &self.0 {
            if !sites.contains(&item.site()) {
                sites.push(item.site());
            }
        }
        sites
    }

    /// The selection after an edit (design §7.11): an atom that no longer
    /// exists as it was becomes its whole site, and removed sites drop out.
    pub fn remap(&self, ids: &IdMap) -> Self {
        let mut items = Vec::new();
        for item in &self.0 {
            let mapped: Vec<Selected> = match *item {
                // A site split into P1 sites selects them all.
                Selected::Site(site) => ids.sites(site).into_iter().map(Selected::Site).collect(),
                Selected::Atom(atom) => match ids.atom(atom) {
                    AtomFate::Kept(atom) => vec![Selected::Atom(atom)],
                    AtomFate::Site(site) => vec![Selected::Site(site)],
                    AtomFate::Removed => Vec::new(),
                },
            };
            for mapped in mapped {
                if !items.contains(&mapped) {
                    items.push(mapped);
                }
            }
        }
        Self(items)
    }

    /// Adds the atom, or removes it if it is selected by itself.
    pub fn toggle_atom(&mut self, atom: AtomRef) {
        let item = Selected::Atom(atom);
        if let Some(ix) = self.0.iter().position(|selected| *selected == item) {
            self.0.remove(ix);
        } else {
            self.0.push(item);
        }
    }
}

impl StructureWorkspace {
    /// The site shown in the properties panel.
    pub(crate) fn selected_site(&self) -> Option<SiteId> {
        self.selection.primary_site()
    }

    pub(crate) fn set_selection(
        &mut self,
        selection: Selection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selection == selection {
            return;
        }
        let site_changed = selection.primary_site() != self.selection.primary_site();
        self.selection = selection;
        if site_changed {
            self.sync_site_inputs(window, cx);
        }
        self.sync_viewport_selection(cx);
        cx.notify();
    }

    /// Rings the atoms the viewport should mark: the measurement picks while
    /// measuring, the selection otherwise.
    pub(crate) fn sync_viewport_selection(&self, cx: &mut Context<Self>) {
        let marked = |atom: AtomRef| {
            if self.measuring {
                self.measure_picks.contains(&atom)
            } else {
                self.selection.contains_atom(atom)
            }
        };
        let spheres: Vec<usize> = self
            .document
            .derived()
            .atoms
            .iter()
            .enumerate()
            .filter_map(|(ix, atom)| marked(atom.atom).then_some(ix))
            .collect();
        self.viewport
            .update(cx, |viewport, cx| viewport.set_selection(spheres, cx));
    }

    /// The atom a viewport pick refers to; spheres are in the order of the
    /// document's displayed atoms.
    pub(crate) fn picked_atom(&self, pick: Option<Pick>) -> Option<AtomRef> {
        match pick? {
            Pick::Sphere(ix) => self.document.derived().atoms.get(ix).map(|atom| atom.atom),
            Pick::Bond(_) => None,
        }
    }

    /// Where the displayed atom at a fractional position appears in the
    /// window, for driving the viewport with synthetic pointer events (the
    /// screenshot tour). `None` if no such atom is displayed.
    pub fn displayed_atom_position(&self, fractional: [f64; 3], cx: &App) -> Option<Point<Pixels>> {
        let derived = self.document.derived();
        let atom = derived.atoms.iter().find(|atom| {
            derived
                .fractional(atom.atom)
                .is_some_and(|f| (0..3).all(|i| (f[i] - fractional[i]).abs() < 1e-3))
        })?;
        let viewport = self.viewport.read(cx);
        Some(viewport.bounds()?.origin + viewport.project(atom.position)?)
    }

    pub(crate) fn on_viewport_event(
        &mut self,
        _: &Entity<Viewport>,
        event: &ViewportEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ViewportEvent::Clicked { pick, modifiers } = *event else {
            return;
        };
        let atom = self.picked_atom(pick);
        if self.measuring {
            // A click on a bond neither picks nor resets.
            if atom.is_some() || pick.is_none() {
                self.measure_pick(atom, window, cx);
            }
            return;
        }
        let adding = modifiers.shift || modifiers.platform;
        let selection = match (atom, pick) {
            (Some(atom), _) if adding => {
                let mut selection = self.selection.clone();
                selection.toggle_atom(atom);
                selection
            }
            (Some(atom), _) => Selection::atom(atom),
            (None, None) if !adding => Selection::default(),
            _ => return,
        };
        self.set_selection(selection, window, cx);
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's glob would shadow the `test` attribute.
    use mw_crystal::sample::{self, SampleKey};
    use mw_crystal::{AtomRef, CrystalDocument, Edit, SiteId, Target};

    use super::Selection;

    fn atom(site: u32, member: u16) -> AtomRef {
        AtomRef::new(SiteId(site), member, [0; 3])
    }

    #[test]
    fn sites_select_all_their_atoms() {
        let selection = Selection::site(SiteId(2));
        assert!(selection.contains_atom(atom(2, 3)));
        assert!(!selection.contains_atom(atom(1, 0)));
        assert_eq!(selection.primary_site(), Some(SiteId(2)));
    }

    #[test]
    fn toggling_adds_and_removes_atoms() {
        let mut selection = Selection::atom(atom(1, 0));
        selection.toggle_atom(atom(2, 1));
        assert!(selection.touches_site(SiteId(1)) && selection.touches_site(SiteId(2)));
        assert_eq!(selection.primary_site(), Some(SiteId(2)));
        // Another image of the same orbit member is a different atom.
        let image = atom(2, 1).translated([1, 0, 0]);
        assert!(!selection.contains_atom(image));

        selection.toggle_atom(atom(2, 1));
        assert_eq!(selection.primary_site(), Some(SiteId(1)));
        selection.toggle_atom(atom(1, 0));
        assert_eq!(selection, Selection::default());
    }

    #[test]
    fn selections_follow_edits() {
        let sample = sample::load(SampleKey::StrontiumTitanate);
        let mut document = CrystalDocument::with_derived(sample.structure, sample.derived);
        let mut selection = Selection::atom(atom(1, 0));
        selection.toggle_atom(atom(3, 2));
        selection.0.push(super::Selected::Site(SiteId(2)));
        assert_eq!(
            selection.targets()[1],
            Target::Atom(atom(3, 2)),
            "atoms stay atoms"
        );
        assert_eq!(selection.sites(), [SiteId(1), SiteId(3), SiteId(2)]);

        let outcome = document.edit(Edit::RemoveSite { site: SiteId(3) }).unwrap();
        let remapped = selection.remap(&outcome.ids);
        assert_eq!(remapped.sites(), [SiteId(1), SiteId(2)]);
        assert!(remapped.contains_atom(atom(1, 0)));
        // Nothing else changed: the same selection.
        let outcome = document.undo().unwrap();
        assert_eq!(remapped.remap(&outcome.ids), remapped);
    }
}
