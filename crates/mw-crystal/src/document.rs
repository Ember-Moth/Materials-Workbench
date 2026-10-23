//! A structure with what is attached to it (design §6): its measurements and
//! the commands applied to it, with undo and redo (design §7.12).
//!
//! The undo stack lives here until a second kind of document needs one; it
//! does not depend on what the commands are.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    AtomFate, Derived, Edit, EditError, EditMode, EditWarning, IdMap, Measurement, SiteId,
    Structure, Target, edit,
};

/// A command on a document: a structure edit, or a change to its
/// measurements. Every command is one undo step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Edit(Edit),
    AddMeasurement(Measurement),
    RemoveMeasurement(Measurement),
}

/// A measurement kept with the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Annotation {
    pub measurement: Measurement,
    /// An atom it refers to was removed, or its site reshaped so that the
    /// reference may name another atom. Stale measurements are kept, not
    /// dropped (design §7.11).
    pub stale: bool,
}

/// What a command, undo or redo did.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Outcome {
    /// `false` when the command set what was already there; it then adds
    /// no undo step.
    pub changed: bool,
    /// For updating selections (design §7.11).
    pub ids: IdMap,
    pub warnings: Vec<EditWarning>,
}

#[derive(Clone, Debug)]
struct Step {
    /// Unique within the document, for telling the saved state apart.
    id: u64,
    command: Command,
    /// For a structure edit, the edits that undo and redo it. Each is the
    /// inverse the other gave when it was last applied, so both restore
    /// exactly.
    edits: Option<(Edit, Edit)>,
    /// The measurements before and after the step.
    before: Vec<Annotation>,
    after: Vec<Annotation>,
}

/// One structure document: the structure, its derived data (always up to
/// date), its measurements and its undo history.
#[derive(Clone, Debug)]
pub struct CrystalDocument {
    structure: Arc<Structure>,
    derived: Derived,
    annotations: Vec<Annotation>,
    revision: u64,
    done: Vec<Step>,
    undone: Vec<Step>,
    next_step: u64,
    /// The last step done when the document was saved, `None` for none.
    /// After undoing past it and doing something else, no state matches it
    /// any more.
    saved: Option<u64>,
}

const INVERSE: &str = "an inverse applies to the structure it was made from";

impl CrystalDocument {
    pub fn new(structure: Structure) -> Self {
        let derived = Derived::compute(&structure);
        Self::with_derived(structure, derived)
    }

    /// A document for a structure whose derived data is already computed.
    pub fn with_derived(structure: Structure, derived: Derived) -> Self {
        Self {
            structure: Arc::new(structure),
            derived,
            annotations: Vec::new(),
            revision: 0,
            done: Vec::new(),
            undone: Vec::new(),
            next_step: 0,
            saved: None,
        }
    }

    /// A document as it was saved: the structure and its measurements, with
    /// no undo history. A measurement whose atoms no longer exist is marked
    /// stale rather than dropped.
    pub fn restore(structure: Structure, annotations: Vec<Annotation>) -> Self {
        let mut document = Self::new(structure);
        document.annotations = annotations
            .into_iter()
            .map(|mut annotation| {
                if document.check_measurement(&annotation.measurement).is_err() {
                    annotation.stale = true;
                }
                annotation
            })
            .collect();
        document
    }

    /// Whether the document differs from its last saved state (or, if it
    /// was never saved, from how it was made). Undoing back to that state
    /// makes it unmodified again.
    pub fn is_modified(&self) -> bool {
        self.done.last().map(|step| step.id) != self.saved
    }

    /// Records the current state as saved.
    pub fn mark_saved(&mut self) {
        self.saved = self.done.last().map(|step| step.id);
    }

    pub fn structure(&self) -> &Structure {
        &self.structure
    }

    /// The current structure, to work on away from the document; edits
    /// make a new one and leave this one as it is.
    pub fn snapshot(&self) -> Arc<Structure> {
        self.structure.clone()
    }

    pub fn derived(&self) -> &Derived {
        &self.derived
    }

    pub fn annotations(&self) -> &[Annotation] {
        &self.annotations
    }

    /// Counts changes to the document, undo and redo included.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn edit_mode(&self) -> EditMode {
        self.structure.edit_mode()
    }

    /// The sites an edit of `targets` acts on; see [`edit::target_sites`].
    pub fn target_sites(&self, targets: &[Target]) -> Result<Vec<SiteId>, EditError> {
        edit::target_sites(&self.structure, &self.derived, targets)
    }

    pub fn edit(&mut self, edit: Edit) -> Result<Outcome, EditError> {
        self.execute(Command::Edit(edit))
    }

    /// Runs a command as one undo step and clears the redo history. On
    /// error, nothing changes.
    pub fn execute(&mut self, command: Command) -> Result<Outcome, EditError> {
        let before = self.annotations.clone();
        let (edits, outcome) = match &command {
            Command::Edit(edit) => {
                let applied = edit.apply(&self.structure)?;
                if applied.structure == *self.structure {
                    return Ok(Outcome {
                        warnings: applied.warnings,
                        ..Outcome::default()
                    });
                }
                self.set_structure(applied.structure);
                for annotation in &mut self.annotations {
                    annotation.remap(&applied.ids);
                }
                let outcome = Outcome {
                    changed: true,
                    ids: applied.ids,
                    warnings: applied.warnings,
                };
                (Some((applied.inverse, edit.clone())), outcome)
            }
            Command::AddMeasurement(measurement) => {
                self.check_measurement(measurement)?;
                if self
                    .annotations
                    .iter()
                    .any(|a| a.measurement == *measurement)
                {
                    return Ok(Outcome::default());
                }
                self.annotations.push(Annotation {
                    measurement: *measurement,
                    stale: false,
                });
                self.revision += 1;
                (None, Outcome::changed())
            }
            Command::RemoveMeasurement(measurement) => {
                let index = self
                    .annotations
                    .iter()
                    .position(|a| a.measurement == *measurement)
                    .ok_or(EditError::UnknownMeasurement)?;
                self.annotations.remove(index);
                self.revision += 1;
                (None, Outcome::changed())
            }
        };
        self.done.push(Step {
            id: self.next_step,
            command,
            edits,
            before,
            after: self.annotations.clone(),
        });
        self.next_step += 1;
        self.undone.clear();
        Ok(outcome)
    }

    /// Undoes the last step, if any.
    pub fn undo(&mut self) -> Option<Outcome> {
        let mut step = self.done.pop()?;
        let outcome = match &mut step.edits {
            Some((undo, redo)) => {
                let applied = undo.apply(&self.structure).expect(INVERSE);
                *redo = applied.inverse;
                self.set_structure(applied.structure);
                Outcome {
                    changed: true,
                    ids: applied.ids,
                    warnings: applied.warnings,
                }
            }
            None => {
                self.revision += 1;
                Outcome::changed()
            }
        };
        self.annotations = step.before.clone();
        self.undone.push(step);
        Some(outcome)
    }

    /// Redoes the last undone step, if any.
    pub fn redo(&mut self) -> Option<Outcome> {
        let mut step = self.undone.pop()?;
        let outcome = match &mut step.edits {
            Some((undo, redo)) => {
                let applied = redo.apply(&self.structure).expect(INVERSE);
                *undo = applied.inverse;
                self.set_structure(applied.structure);
                Outcome {
                    changed: true,
                    ids: applied.ids,
                    warnings: applied.warnings,
                }
            }
            None => {
                self.revision += 1;
                Outcome::changed()
            }
        };
        self.annotations = step.after.clone();
        self.done.push(step);
        Some(outcome)
    }

    /// The command that [`Self::undo`] would undo.
    pub fn undo_command(&self) -> Option<&Command> {
        self.done.last().map(|step| &step.command)
    }

    /// The command that [`Self::redo`] would redo.
    pub fn redo_command(&self) -> Option<&Command> {
        self.undone.last().map(|step| &step.command)
    }

    /// The commands that made the current state, oldest first: the record
    /// for provenance and replay (design §7.12).
    pub fn history(&self) -> impl Iterator<Item = &Command> {
        self.done.iter().map(|step| &step.command)
    }

    fn set_structure(&mut self, structure: Structure) {
        self.derived = Derived::compute(&structure);
        self.structure = Arc::new(structure);
        self.revision += 1;
    }

    fn check_measurement(&self, measurement: &Measurement) -> Result<(), EditError> {
        let atoms = measurement.atoms();
        for (i, atom) in atoms.iter().enumerate() {
            if self.derived.fractional(*atom).is_none() {
                return Err(EditError::UnknownAtom(*atom));
            }
            if atoms[..i].contains(atom) {
                return Err(EditError::RepeatedAtom(*atom));
            }
        }
        Ok(())
    }
}

impl Outcome {
    fn changed() -> Self {
        Self {
            changed: true,
            ..Self::default()
        }
    }
}

impl Annotation {
    /// Follows the atoms through an edit; any that no longer exists as it
    /// was makes the measurement stale.
    fn remap(&mut self, ids: &IdMap) {
        for atom in self.measurement.atoms_mut() {
            match ids.atom(*atom) {
                AtomFate::Kept(kept) => *atom = kept,
                AtomFate::Site(_) | AtomFate::Removed => self.stale = true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{self, SampleKey};
    use crate::{AtomRef, CellParam, CellValues, Measured, Species};

    fn document(key: SampleKey) -> CrystalDocument {
        let sample = sample::load(key);
        CrystalDocument::with_derived(sample.structure, sample.derived)
    }

    fn atom(document: &CrystalDocument, site: usize, member: u16) -> AtomRef {
        AtomRef::new(document.structure().sites[site].id, member, [0; 3])
    }

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut doc = document(SampleKey::StrontiumTitanate);
        let original = doc.snapshot();
        let ti = doc.structure().sites[1].id;

        let a = CellValues::default().with(CellParam::A, 4.0);
        doc.edit(Edit::SetCell { values: a }).unwrap();
        doc.edit(Edit::RemoveSite { site: ti }).unwrap();
        let edited = doc.snapshot();
        assert_eq!(doc.revision(), 2);
        assert_eq!(doc.derived().atoms_in_cell(), 4);
        assert_eq!(doc.history().count(), 2);
        assert_eq!(
            doc.undo_command(),
            Some(&Command::Edit(Edit::RemoveSite { site: ti }))
        );

        doc.undo().unwrap();
        assert_eq!(doc.derived().atoms_in_cell(), 5);
        doc.undo().unwrap();
        assert_eq!(*doc.structure(), *original);
        assert_eq!(doc.undo(), None);
        assert_eq!(doc.revision(), 4);

        doc.redo().unwrap();
        doc.redo().unwrap();
        assert_eq!(*doc.structure(), *edited);
        assert_eq!(doc.redo(), None);
        // The snapshots taken along the way did not change.
        assert_eq!(original.sites.len(), 3);

        // A new command drops what was undone.
        doc.undo().unwrap();
        let o = doc.structure().sites[2].id;
        let species = Species::element("F");
        doc.edit(Edit::SetSpecies {
            site: o,
            occupant: "O1".into(),
            species,
        })
        .unwrap();
        assert_eq!(doc.redo_command(), None);
        assert_eq!(doc.history().count(), 2);
    }

    #[test]
    fn edits_that_change_nothing_add_no_step() {
        let mut doc = document(SampleKey::Rutile);
        let same = CellValues::default().with(CellParam::A, 4.594);
        let outcome = doc.edit(Edit::SetCell { values: same }).unwrap();
        assert!(!outcome.changed);
        assert_eq!((doc.revision(), doc.history().count()), (0, 0));
    }

    #[test]
    fn refused_edits_change_nothing() {
        let mut doc = document(SampleKey::Rutile);
        let error = doc
            .edit(Edit::SetOccupancy {
                site: doc.structure().sites[0].id,
                occupant: "Ti1".into(),
                occupancy: Measured::exact(2.0),
            })
            .unwrap_err();
        assert_eq!(error.code(), "occupancy_out_of_range");
        assert_eq!((doc.revision(), doc.undo()), (0, None));
    }

    #[test]
    fn measurements_go_stale_and_come_back() {
        let mut doc = document(SampleKey::StrontiumTitanate);
        let ti_o = Measurement::Distance([atom(&doc, 1, 0), atom(&doc, 2, 0)]);
        let sr_o = Measurement::Distance([atom(&doc, 0, 0), atom(&doc, 2, 1)]);
        doc.execute(Command::AddMeasurement(ti_o)).unwrap();
        doc.execute(Command::AddMeasurement(sr_o)).unwrap();
        // Adding it again changes nothing.
        let again = doc.execute(Command::AddMeasurement(ti_o)).unwrap();
        assert!(!again.changed);

        let ti = doc.structure().sites[1].id;
        let outcome = doc.edit(Edit::RemoveSite { site: ti }).unwrap();
        assert_eq!(outcome.ids.site(ti), None);
        let stale: Vec<bool> = doc.annotations().iter().map(|a| a.stale).collect();
        assert_eq!(stale, [true, false]);
        assert_eq!(
            doc.annotations()[0]
                .measurement
                .value(&doc.structure().lattice, doc.derived()),
            None
        );

        doc.undo().unwrap();
        assert!(doc.annotations().iter().all(|a| !a.stale));
        doc.redo().unwrap();
        assert!(doc.annotations()[0].stale);

        // Removing a measurement is a step of its own.
        doc.execute(Command::RemoveMeasurement(sr_o)).unwrap();
        assert_eq!(doc.annotations().len(), 1);
        doc.undo().unwrap();
        assert_eq!(doc.annotations()[1].measurement, sr_o);
        assert_eq!(
            doc.execute(Command::RemoveMeasurement(Measurement::Distance([
                atom(&doc, 0, 0),
                atom(&doc, 0, 0).translated([1, 0, 0])
            ]))),
            Err(EditError::UnknownMeasurement)
        );
    }

    #[test]
    fn measurements_must_name_existing_distinct_atoms() {
        let mut doc = document(SampleKey::StrontiumTitanate);
        let sr = atom(&doc, 0, 0);
        let missing = atom(&doc, 2, 5);
        assert_eq!(
            doc.execute(Command::AddMeasurement(Measurement::Distance([
                sr, missing
            ]))),
            Err(EditError::UnknownAtom(missing))
        );
        assert_eq!(
            doc.execute(Command::AddMeasurement(Measurement::Angle([
                sr,
                atom(&doc, 1, 0),
                sr
            ]))),
            Err(EditError::RepeatedAtom(sr))
        );
        let image = sr.translated([1, 0, 0]);
        let outcome = doc
            .execute(Command::AddMeasurement(Measurement::Distance([sr, image])))
            .unwrap();
        assert!(outcome.changed);
    }

    #[test]
    fn the_saved_state_survives_undo_and_redo() {
        let mut doc = document(SampleKey::StrontiumTitanate);
        assert!(!doc.is_modified());
        let a = |value| Edit::SetCell {
            values: CellValues::default().with(CellParam::A, value),
        };
        doc.edit(a(4.0)).unwrap();
        assert!(doc.is_modified());
        doc.undo().unwrap();
        assert!(!doc.is_modified());
        doc.redo().unwrap();
        doc.mark_saved();
        assert!(!doc.is_modified());
        doc.edit(a(4.1)).unwrap();
        assert!(doc.is_modified());
        doc.undo().unwrap();
        assert!(!doc.is_modified());
        // Undoing past the saved state and doing something else leaves no
        // way back to it but undoing further.
        doc.undo().unwrap();
        assert!(doc.is_modified());
        doc.edit(a(4.0)).unwrap();
        assert!(doc.is_modified());
        assert_eq!(doc.structure().lattice.params().a.value(), 4.0);
    }

    #[test]
    fn restored_measurements_of_missing_atoms_are_stale() {
        let doc = document(SampleKey::StrontiumTitanate);
        let sr = atom(&doc, 0, 0);
        let ti = atom(&doc, 1, 0);
        let gone = AtomRef::new(SiteId(9), 0, [0; 3]);
        let annotation = |atoms| Annotation {
            measurement: Measurement::Distance(atoms),
            stale: false,
        };
        let restored = CrystalDocument::restore(
            doc.structure().clone(),
            vec![annotation([sr, ti]), annotation([sr, gone])],
        );
        let stale: Vec<bool> = restored.annotations().iter().map(|a| a.stale).collect();
        assert_eq!(stale, [false, true]);
        assert!(!restored.is_modified());
        assert_eq!(restored.history().count(), 0);
    }

    #[test]
    fn commands_serialize() {
        let commands = [
            Command::Edit(Edit::RemoveSite { site: SiteId(3) }),
            Command::AddMeasurement(Measurement::Angle([
                AtomRef::new(SiteId(1), 0, [0, 0, 0]),
                AtomRef::new(SiteId(2), 1, [0, -1, 0]),
                AtomRef::new(SiteId(1), 3, [1, 0, 0]),
            ])),
        ];
        for command in commands {
            let json = serde_json::to_string(&command).unwrap();
            assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), command);
        }
    }
}
