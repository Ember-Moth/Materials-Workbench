//! The manifest `project.json` (design §5) and the provenance of its
//! entries (design §7).

use std::fmt;

use mw_crystal_io::{ImportedBlock, cif};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Sha256, Timestamp};

/// Identifies a project wherever it is moved; made when it is created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(Uuid);

/// Identifies an entry within its project, and names its files. Never
/// reused. Sessions refer to entries by their own short ids; these stay in
/// the files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(Uuid);

macro_rules! uuid_id {
    ($name:ident) => {
        impl $name {
            /// A new random id.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// A fixed id, for reproducible files such as the samples.
            pub const fn from_u128(value: u128) -> Self {
                Self(Uuid::from_u128(value))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.hyphenated().fmt(f)
            }
        }
    };
}

uuid_id!(ProjectId);
uuid_id!(EntryId);

impl EntryId {
    /// The id a file is named after.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Uuid::try_parse(text).ok().map(Self)
    }
}

/// What a project holds: its identity, name and entries, in the order the
/// sidebar lists them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Manifest {
    pub id: ProjectId,
    pub name: String,
    pub entries: Vec<Entry>,
}

impl Manifest {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: ProjectId::new(),
            name: name.into(),
            entries: Vec::new(),
        }
    }

    pub fn entry(&self, id: EntryId) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}

/// One object of a project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Entry {
    pub id: EntryId,
    pub kind: EntryKind,
    pub name: String,
    pub provenance: Provenance,
}

impl Entry {
    pub fn new(
        id: EntryId,
        kind: EntryKind,
        name: impl Into<String>,
        provenance: Provenance,
    ) -> Self {
        Self {
            id,
            kind,
            name: name.into(),
            provenance,
        }
    }
}

/// What kind of object an entry is; each kind has its own directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// `structures/<id>.json`, a [`crate::StructureFile`].
    CrystalStructure,
}

/// Where an entry came from (design §7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Import(Import),
    Derived(Derived),
    Created(Created),
}

impl Provenance {
    pub fn import(&self) -> Option<&Import> {
        match self {
            Self::Import(import) => Some(import),
            Self::Derived(_) | Self::Created(_) => None,
        }
    }
}

/// Read from a file, whose copy the project keeps (design §7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Import {
    pub format: SourceFormat,
    pub file_name: String,
    /// The file's path when it was imported, as a record; never read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Of the file's bytes; also the name of its copy.
    pub sha256: Sha256,
    /// The data block the structure was read from.
    pub block: String,
    pub imported_at: Timestamp,
    /// The structure as imported ([`crate::structure_sha256`]), to tell
    /// whether it has been edited since.
    pub structure_sha256: Sha256,
}

impl Import {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        format: SourceFormat,
        file_name: impl Into<String>,
        path: Option<String>,
        sha256: Sha256,
        block: impl Into<String>,
        imported_at: Timestamp,
        structure_sha256: Sha256,
    ) -> Self {
        Self {
            format,
            file_name: file_name.into(),
            path,
            sha256,
            block: block.into(),
            imported_at,
            structure_sha256,
        }
    }

    /// The copy's path inside the project directory.
    pub fn copy_path(&self) -> String {
        format!("sources/{}.{}", self.sha256, self.format.extension())
    }

    /// Reads the copy's bytes again and returns the data block the entry
    /// came from, with its diagnostics (the file's own first, as on import)
    /// and the fields the structure does not hold (design §7.1).
    pub fn read_block(&self, bytes: &[u8]) -> Option<ImportedBlock> {
        match self.format {
            SourceFormat::Cif => {
                let report = cif::read(bytes);
                let mut block = report
                    .blocks
                    .into_iter()
                    .find(|block| block.name == self.block)?;
                block.diagnostics.splice(0..0, report.diagnostics);
                Some(block)
            }
        }
    }
}

/// The formats files are imported from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    Cif,
}

impl SourceFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Cif => "cif",
        }
    }
}

/// Made from another entry by an operation (design §7.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Derived {
    /// The entry it was made from; the record stays when that is deleted.
    pub from: EntryId,
    /// That entry's name at the time, for when it is gone.
    pub from_name: String,
    /// That entry's structure at the time.
    pub from_structure_sha256: Sha256,
    pub operation: Operation,
    pub derived_at: Timestamp,
}

impl Derived {
    pub fn new(
        from: EntryId,
        from_name: impl Into<String>,
        from_structure_sha256: Sha256,
        operation: Operation,
        derived_at: Timestamp,
    ) -> Self {
        Self {
            from,
            from_name: from_name.into(),
            from_structure_sha256,
            operation,
            derived_at,
        }
    }
}

/// An operation that makes a new entry, with everything needed to repeat it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Operation {
    /// The new lattice is `A·M` (crystallography design §7.9); rows of M.
    MakeSupercell { matrix: [[i32; 3]; 3] },
}

/// Made from nothing in the application.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Created {
    pub created_at: Timestamp,
}

impl Created {
    pub fn new(created_at: Timestamp) -> Self {
        Self { created_at }
    }
}
