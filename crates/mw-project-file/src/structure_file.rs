use mw_crystal::{Annotation, Command, Structure};
use serde::{Deserialize, Serialize};

/// What `structures/<id>.json` holds (design §6): the structure, its
/// measurements and the record of the commands applied to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StructureFile {
    pub structure: Structure,
    #[serde(default)]
    pub measurements: Vec<Annotation>,
    /// Committed commands since the structure was imported or derived, in
    /// order; a record, never replayed (design §7.3).
    #[serde(default)]
    pub edits: Vec<Command>,
}

impl StructureFile {
    pub fn new(structure: Structure, measurements: Vec<Annotation>, edits: Vec<Command>) -> Self {
        Self {
            structure,
            measurements,
            edits,
        }
    }
}
