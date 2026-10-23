//! Editing the open structure (crystallography design §7): the workspace
//! runs domain commands on its document, then brings the selection, the
//! measurement in progress, the scene and the fields up to date. Undo and
//! redo go through the same path.

use gpui_kit::component::WindowExt;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;
use mw_crystal::{
    AtomFate, CellParam, CellValues, Command, Edit, EditError, EditWarning, Measured, Outcome,
    SiteId, Species, Structure,
};

use crate::actions::{AddMixedOccupant, DeleteSelection, Redo, Undo};
use crate::workspace::StructureWorkspace;
use crate::{dialogs, format};

fn site_label(structure: &Structure, site: SiteId) -> String {
    structure
        .site(site)
        .map_or_else(|| site.0.to_string(), |s| s.label().to_string())
}

fn param_name(param: CellParam) -> &'static str {
    match param {
        CellParam::A => "a",
        CellParam::B => "b",
        CellParam::C => "c",
        CellParam::Alpha => "α",
        CellParam::Beta => "β",
        CellParam::Gamma => "γ",
    }
}

/// What a command does, for the undo and redo tooltips.
pub(crate) fn command_name(command: &Command) -> SharedString {
    match command {
        Command::Edit(edit) => match edit {
            Edit::SetCell { .. } | Edit::RestoreCell { .. } => tr!("edit.command.set_cell"),
            Edit::SetSpecies { .. } => tr!("edit.command.set_species"),
            Edit::SetOccupancy { .. } => tr!("edit.command.set_occupancy"),
            Edit::AddOccupant { .. } | Edit::RestoreOccupant { .. } => {
                tr!("edit.command.add_occupant")
            }
            Edit::RemoveOccupant { .. } => tr!("edit.command.remove_occupant"),
            Edit::RenameLabel { .. } => tr!("edit.command.rename_label"),
            Edit::SetPosition { .. } => tr!("edit.command.set_position"),
            // Batches only delete several sites at once so far.
            Edit::RemoveSite { .. } | Edit::RestoreSite { .. } | Edit::Batch { .. } => {
                tr!("edit.command.remove_site")
            }
        },
        Command::AddMeasurement(_) => tr!("edit.command.add_measurement"),
        Command::RemoveMeasurement(_) => tr!("edit.command.remove_measurement"),
    }
}

/// Why an edit was refused, in the interface language. `structure` is the
/// one the edit was tried on.
pub(crate) fn error_text(error: &EditError, structure: &Structure) -> SharedString {
    let label = |site: SiteId| site_label(structure, site);
    match error {
        EditError::BreaksSymmetry {
            site, equivalent, ..
        } => tr!(
            "edit.error.breaks_symmetry",
            label = label(*site),
            count = equivalent
        ),
        EditError::FixedCellParameter { param } => {
            tr!(
                "edit.error.fixed_cell_parameter",
                param = param_name(*param)
            )
        }
        EditError::LinkedCellParameters { param, other } => tr!(
            "edit.error.linked_cell_parameters",
            param = param_name(*param),
            other = param_name(*other)
        ),
        EditError::InvalidCell => tr!("edit.error.invalid_cell"),
        EditError::InvalidPosition => tr!("edit.error.not_a_number"),
        EditError::OccupancyOutOfRange { .. } => tr!("edit.error.occupancy_out_of_range"),
        EditError::LastOccupant { site } => {
            tr!("edit.error.last_occupant", label = label(*site))
        }
        EditError::UnknownElement { symbol } => {
            tr!("edit.error.unknown_element", symbol = symbol)
        }
        EditError::InvalidLabel { label } => tr!("edit.error.invalid_label", label = label),
        EditError::DuplicateLabel { label } => {
            tr!("edit.error.duplicate_label", label = label)
        }
        // The rest name sites, atoms or measurements that the interface
        // only offers while they exist.
        other => tr!("edit.error.other", reason = other.to_string()),
    }
}

/// A warning after an edit, in the interface language.
pub(crate) fn warning_text(warning: &EditWarning, structure: &Structure) -> SharedString {
    match warning {
        EditWarning::OccupancySum { site, sum } => tr!(
            "edit.warning.occupancy_sum",
            label = site_label(structure, *site),
            sum = format::occupancy(*sum)
        ),
        EditWarning::ShortContact {
            site,
            other,
            distance,
        } => tr!(
            "edit.warning.short_contact",
            a = site_label(structure, *site),
            b = site_label(structure, *other),
            distance = format::distance(*distance)
        ),
        other => SharedString::from(other.to_string()),
    }
}

/// A typed number, or `None` with an error shown.
pub(crate) fn parse_number(text: &str, window: &mut Window, cx: &mut App) -> Option<f64> {
    let value = text.trim().parse::<f64>().ok().filter(|v| v.is_finite());
    if value.is_none() {
        window.push_notification(Notification::error(tr!("edit.error.not_a_number")), cx);
    }
    value
}

impl StructureWorkspace {
    /// Runs a command on the document as one undo step and updates the
    /// view: the fields, the scene, and the selection, which follows the
    /// change. A refused command changes nothing and says why in a
    /// notification. The entry point for everything that edits the open
    /// structure, the interface's own fields included.
    pub fn execute(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Outcome, EditError> {
        match self.document.crystal.execute(command) {
            Ok(outcome) => {
                self.after_change(&outcome, window, cx);
                Ok(outcome)
            }
            Err(error) => {
                let text = error_text(&error, self.document.structure());
                window.push_notification(Notification::error(text), cx);
                Err(error)
            }
        }
    }

    pub(crate) fn edit(
        &mut self,
        edit: Edit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Outcome, EditError> {
        self.execute(Command::Edit(edit), window, cx)
    }

    /// Follows the selection and the atoms picked for a measurement through
    /// the change (design §7.11), shows its warnings, and redraws.
    fn after_change(&mut self, outcome: &Outcome, window: &mut Window, cx: &mut Context<Self>) {
        if !outcome.changed {
            return;
        }
        let structure = self.document.structure();
        for warning in &outcome.warnings {
            window.push_notification(Notification::warning(warning_text(warning, structure)), cx);
        }
        let ids = &outcome.ids;
        self.measure_picks.retain_mut(|atom| match ids.atom(*atom) {
            AtomFate::Kept(kept) => {
                *atom = kept;
                true
            }
            AtomFate::Site(_) | AtomFate::Removed => false,
        });
        self.selection = self.selection.remap(ids);
        self.sync_cell_inputs(window, cx);
        self.sync_site_inputs(window, cx);
        self.refresh_scene(cx);
        cx.notify();
    }

    pub(crate) fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut outcome) = self.document.crystal.undo() {
            // What the edit warned about was shown when it was made.
            outcome.warnings.clear();
            self.after_change(&outcome, window, cx);
        }
    }

    pub(crate) fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut outcome) = self.document.crystal.redo() {
            // What the edit warned about was shown when it was made.
            outcome.warnings.clear();
            self.after_change(&outcome, window, cx);
        }
    }

    /// Opens the dialog that adds an element to the selected site.
    pub(crate) fn add_mixed_occupant(
        &mut self,
        _: &AddMixedOccupant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entity = cx.entity();
        dialogs::open_add_occupant(self, entity, window, cx);
    }

    /// Delete: whole sites go at once and can be undone. Some but not all
    /// equivalent atoms of a site would break the symmetry, so the dialog
    /// offers the ways forward instead (design §7.1).
    pub(crate) fn delete_selection(
        &mut self,
        _: &DeleteSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.selection.targets();
        if targets.is_empty() {
            return;
        }
        match self.document.crystal.target_sites(&targets) {
            Ok(sites) => self.delete_sites(sites, window, cx),
            Err(error @ EditError::BreaksSymmetry { .. }) => {
                let sites = self.selection.sites();
                let entity = cx.entity();
                dialogs::open_partial_delete(self, entity, &error, sites, window, cx);
            }
            Err(error) => {
                let text = error_text(&error, self.document.structure());
                window.push_notification(Notification::error(text), cx);
            }
        }
    }

    /// Deletes sites with all their equivalent atoms, as one undo step.
    pub(crate) fn delete_sites(
        &mut self,
        sites: Vec<SiteId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let derived = self.document.derived();
        let atoms: usize = sites
            .iter()
            .map(|&site| derived.multiplicity(site).unwrap_or(0))
            .sum();
        let message = match sites[..] {
            [site] => tr!(
                "edit.deleted_site",
                label = site_label(self.document.structure(), site),
                count = atoms
            ),
            _ => tr!("edit.deleted_sites", sites = sites.len(), count = atoms),
        };
        let mut edits: Vec<Edit> = sites
            .into_iter()
            .map(|site| Edit::RemoveSite { site })
            .collect();
        let edit = match edits.len() {
            0 => return,
            1 => edits.remove(0),
            _ => Edit::Batch { edits },
        };
        if self.edit(edit, window, cx).is_ok() {
            window.push_notification(Notification::info(message), cx);
        }
    }

    /// Commits a typed cell parameter. Linked parameters follow (design
    /// §7.3); text that still shows the current value changes nothing.
    pub(crate) fn commit_cell(
        &mut self,
        param: CellParam,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, input)) = self.cell_inputs.iter().find(|(p, _)| *p == param) else {
            return;
        };
        let text = input.read(cx).value();
        let current = self
            .document
            .structure()
            .lattice
            .params()
            .get(param)
            .value();
        if text.trim() == format::cell_param(param, current) {
            return;
        }
        let edit = parse_number(&text, window, cx).map(|value| Edit::SetCell {
            values: CellValues::default().with(param, value),
        });
        if edit.is_none_or(|edit| self.edit(edit, window, cx).is_err()) {
            self.sync_cell_inputs(window, cx);
        }
    }

    /// Commits a typed fractional coordinate of the edited site.
    pub(crate) fn commit_coordinate(
        &mut self,
        axis: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(site) = self
            .editing_site
            .and_then(|id| self.document.structure().site(id))
        else {
            return;
        };
        let (id, mut position) = (site.id, site.position);
        let text = self.coordinate_inputs[axis].read(cx).value();
        if text.trim() == format::coordinate(position[axis]) {
            return;
        }
        let Some(value) = parse_number(&text, window, cx) else {
            self.sync_site_inputs(window, cx);
            return;
        };
        if self.document.crystal.edit_mode() == mw_crystal::EditMode::Symmetric {
            // Mock: moving sites under symmetry, along their special
            // position, comes with v0.2 (design §7.4–7.5).
            dialogs::not_implemented(tr!("feature.move_site"), window, cx);
            self.sync_site_inputs(window, cx);
            return;
        }
        position[axis] = value;
        let edit = Edit::SetPosition { site: id, position };
        if self.edit(edit, window, cx).is_err() {
            self.sync_site_inputs(window, cx);
        }
    }

    /// Commits a typed occupancy of one occupant of the edited site.
    pub(crate) fn commit_occupancy(
        &mut self,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(site) = self
            .editing_site
            .and_then(|id| self.document.structure().site(id))
        else {
            return;
        };
        let Some(occupant) = site.occupants.iter().find(|o| o.label == label.as_ref()) else {
            return;
        };
        let current = occupant.occupancy.value();
        let site = site.id;
        let Some(editor) = self.occupants.iter().find(|e| e.label == label) else {
            return;
        };
        let text = editor.occupancy.read(cx).value();
        if text.trim() == format::occupancy(current) {
            return;
        }
        let edit = parse_number(&text, window, cx).map(|value| Edit::SetOccupancy {
            site,
            occupant: label.to_string(),
            occupancy: Measured::exact(value),
        });
        if edit.is_none_or(|edit| self.edit(edit, window, cx).is_err()) {
            self.sync_site_inputs(window, cx);
        }
    }

    /// Replaces an occupant's element on all equivalent atoms of its site.
    pub(crate) fn set_element(
        &mut self,
        site: SiteId,
        label: SharedString,
        element: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edit = Edit::SetSpecies {
            site,
            occupant: label.to_string(),
            species: Species::element(element),
        };
        if self.edit(edit, window, cx).is_err() {
            self.sync_site_inputs(window, cx);
        }
    }

    pub(crate) fn remove_occupant(
        &mut self,
        site: SiteId,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edit = Edit::RemoveOccupant {
            site,
            occupant: label.to_string(),
        };
        let _ = self.edit(edit, window, cx);
    }

    pub(crate) fn add_occupant(
        &mut self,
        site: SiteId,
        element: &str,
        occupancy: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let edit = Edit::AddOccupant {
            site,
            species: Species::element(element),
            occupancy: Measured::exact(occupancy),
            label: None,
        };
        self.edit(edit, window, cx).is_ok()
    }
}
