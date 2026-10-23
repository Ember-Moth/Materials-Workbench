//! Editing the open structure (crystallography design §7): the workspace
//! runs domain commands on its document, then brings the selection, the
//! measurement in progress, the scene and the fields up to date. Undo and
//! redo go through the same path.

use gpui_kit::component::WindowExt;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;
use mw_crystal::{
    AtomFate, Axis, CellParam, CellValues, Command, Edit, EditError, EditInfo, EditWarning,
    Measured, Outcome, SiteId, Species, Structure, Target, expand_and_remove,
};

use crate::actions::{AddMixedOccupant, AddSite, DeleteSelection, Redo, Undo};
use crate::selection::Selection;
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
            Edit::SetPosition { .. } | Edit::MoveSite { .. } | Edit::RestorePosition { .. } => {
                tr!("edit.command.set_position")
            }
            Edit::SetCoordinate { .. } => tr!("edit.command.set_coordinate"),
            Edit::AddSite { .. } => tr!("edit.command.add_site"),
            Edit::ExpandToP1 | Edit::RestoreStructure { .. } => tr!("edit.command.expand_to_p1"),
            // Batches delete several sites at once, or convert to P1 first.
            Edit::Batch { edits } if matches!(edits.first(), Some(Edit::ExpandToP1)) => {
                tr!("edit.command.expand_and_remove")
            }
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
        EditError::InvalidLabel { label } if label.is_empty() => tr!("edit.error.empty_label"),
        EditError::InvalidLabel { label } => tr!("edit.error.invalid_label", label = label),
        EditError::DuplicateLabel { label } => {
            tr!("edit.error.duplicate_label", label = label)
        }
        EditError::AlreadyP1 => tr!("dialog.convert.already_p1"),
        EditError::FixedCoordinate { site, axis } => tr!(
            "edit.error.fixed_coordinate",
            axis = axis.name(),
            label = label(*site)
        ),
        EditError::CombinedCoordinate { site, axis } => tr!(
            "edit.error.combined_coordinate",
            axis = axis.name(),
            label = label(*site)
        ),
        EditError::CoincidentSite { site } => {
            tr!("edit.error.coincident_site", label = label(*site))
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
        EditWarning::CloseImages { site, distance } => tr!(
            "edit.warning.close_images",
            label = site_label(structure, *site),
            distance = format::distance(*distance)
        ),
        other => SharedString::from(other.to_string()),
    }
}

/// What an edit did besides what was asked, in the interface language.
pub(crate) fn info_text(info: &EditInfo, structure: &Structure) -> SharedString {
    match info {
        EditInfo::Snapped { site, displacement } => tr!(
            "edit.info.snapped",
            label = site_label(structure, *site),
            distance = format::distance(*displacement)
        ),
        EditInfo::Multiplicity {
            site,
            before,
            after,
        } => tr!(
            "edit.info.multiplicity",
            label = site_label(structure, *site),
            before = before,
            after = after
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
        // Last, so on top: what the edit did to the site answers the action
        // directly, e.g. a multiplicity that changed (design §7.4). Each is
        // a sentence of its own; one notification tells them all, a line
        // each.
        if !outcome.info.is_empty() {
            let text: Vec<String> = outcome
                .info
                .iter()
                .map(|info| info_text(info, structure).to_string())
                .collect();
            window.push_notification(Notification::info(text.join("\n")), cx);
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
            outcome.info.clear();
            self.after_change(&outcome, window, cx);
        }
    }

    pub(crate) fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut outcome) = self.document.crystal.redo() {
            // What the edit warned about was shown when it was made.
            outcome.warnings.clear();
            outcome.info.clear();
            self.after_change(&outcome, window, cx);
        }
    }

    /// Opens the dialog that adds a site.
    pub(crate) fn add_site(&mut self, _: &AddSite, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        dialogs::open_add_site(self, entity, window, cx);
    }

    /// Adds a site at typed coordinates (design §7.6) and selects it. One
    /// that would coincide with atoms of an existing site is not added: a
    /// dialog offers the element as a mixed occupant there instead, once
    /// the current dialog has closed. `false` when the site was refused and
    /// the typed values need correcting.
    pub fn add_site_at(
        &mut self,
        position: [f64; 3],
        element: &'static str,
        occupancy: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let edit = Edit::AddSite {
            position,
            species: Species::element(element),
            occupancy: Measured::exact(occupancy),
            label: None,
        };
        if let Err(EditError::CoincidentSite { site }) = edit.apply(self.document.structure()) {
            let entity = cx.entity();
            window.defer(cx, move |window, cx| {
                dialogs::open_coincident_site(entity, site, element, occupancy, window, cx)
            });
            return true;
        }
        if self.edit(edit, window, cx).is_err() {
            return false;
        }
        // A new site goes to the end of the asymmetric unit. Selected, it
        // shows in the list and the 3D view, which is feedback enough.
        if let Some(site) = self.document.structure().sites.last() {
            let id = site.id;
            self.set_selection(Selection::site(id), window, cx);
        }
        true
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
                dialogs::open_partial_delete(self, entity, &error, sites, targets, window, cx);
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

    /// Converts the structure to P1 (design §7.1), as one undo step. The
    /// selection and the measurements follow the atoms.
    pub(crate) fn expand_to_p1(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.edit(Edit::ExpandToP1, window, cx);
    }

    /// Converts the structure to P1 and deletes only what `targets` name,
    /// as one undo step: the delete dialog's way forward (design §7.1).
    pub(crate) fn delete_after_expanding(
        &mut self,
        targets: Vec<Target>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edit = match expand_and_remove(self.document.structure(), &targets) {
            Ok(edit) => edit,
            Err(error) => {
                let text = error_text(&error, self.document.structure());
                window.push_notification(Notification::error(text), cx);
                return;
            }
        };
        let removed = match &edit {
            Edit::Batch { edits } => edits.len() - 1,
            _ => 0,
        };
        if self.edit(edit, window, cx).is_ok() {
            window.push_notification(
                Notification::info(tr!("edit.expanded_and_deleted", count = removed)),
                cx,
            );
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

    /// Commits a typed fractional coordinate of the edited site. Under
    /// symmetry the site stays on its special position: the coordinates
    /// that follow the typed one change with it (design §7.5).
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
        let (id, position) = (site.id, site.position);
        let text = self.coordinate_inputs[axis].read(cx).value();
        if text.trim() == format::coordinate(position[axis]) {
            return;
        }
        let Some(value) = parse_number(&text, window, cx) else {
            self.sync_site_inputs(window, cx);
            return;
        };
        let edit = Edit::SetCoordinate {
            site: id,
            axis: Axis::ALL[axis],
            value,
        };
        match self.edit(edit, window, cx) {
            // A value that rounds to what is shown changes nothing; the
            // field shows it as it was.
            Ok(outcome) if !outcome.changed => self.sync_site_inputs(window, cx),
            Ok(_) => {}
            Err(_) => self.sync_site_inputs(window, cx),
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

    /// Renames the occupant whose fields are at `index` to the typed label.
    /// A refused label (not usable in CIF, or taken) leaves the field showing
    /// the label it had.
    pub(crate) fn commit_label(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(site), Some(editor)) = (self.editing_site, self.occupants.get(index)) else {
            return;
        };
        let current = editor.label.clone();
        let typed = editor.label_input.read(cx).value().trim().to_string();
        if typed == current.as_ref() {
            return;
        }
        let edit = Edit::RenameLabel {
            site,
            occupant: current.to_string(),
            label: typed,
        };
        if self.edit(edit, window, cx).is_err() {
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
