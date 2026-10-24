//! Reading structure files as the interface opens them
//! (`mw_structure::import::OpenedFile`): the format from the file name, the
//! structures named after the file, the file's own diagnostics added to
//! each structure's.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mw_crystal_io::{ImportReport, ImportedBlock, cif, poscar};

use crate::document::Document;
use crate::format::Format;

/// Why a file yielded no report at all.
#[derive(Debug)]
#[non_exhaustive]
pub enum ReadError {
    /// The file's name tells no format that is read, and none was given.
    UnknownFormat { path: PathBuf },
    /// A format that is written but not read.
    NotReadable { format: Format },
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
}

impl ReadError {
    /// Stable, like the registry's error codes.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownFormat { .. } => "unknown_format",
            Self::NotReadable { .. } => "not_readable",
            Self::Io { .. } => "unreadable",
        }
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFormat { path } => write!(
                f,
                "cannot tell the format of {} from its name: .cif is read as CIF; .vasp, \
                 .poscar and names that start with POSCAR or CONTCAR as POSCAR",
                path.display()
            ),
            Self::NotReadable { format } => {
                write!(f, "{} files are written, not read", format.title())
            }
            Self::Io { path, error } => write!(f, "cannot read {}: {error}", path.display()),
        }
    }
}

impl std::error::Error for ReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// One file, read.
#[derive(Clone, Debug)]
pub struct FileReport {
    /// The format the file was read as.
    pub format: Format,
    /// What the importer found: every data block, the failed ones too,
    /// and the problems that belong to no block.
    pub report: ImportReport,
    /// The structures the file holds, in file order, named as the
    /// interface names them.
    pub documents: Vec<Document>,
}

impl FileReport {
    /// Every data block in file order, with the document made from it;
    /// `None` for a block that yielded no structure.
    pub fn blocks(&self) -> impl Iterator<Item = (&ImportedBlock, Option<&Document>)> {
        let mut documents = self.documents.iter();
        self.report.blocks.iter().map(move |block| {
            let document = block.structure.as_ref().and_then(|_| documents.next());
            (block, document)
        })
    }

    /// Blocks that describe a structure the importer could not build.
    pub fn failed_blocks(&self) -> impl Iterator<Item = &ImportedBlock> {
        self.report.failed_blocks()
    }
}

/// Reads a structure file. The format is `format`, or else told from the
/// file's name by the interface's rule ([`Format::of_input`]).
pub fn read_file(path: &Path, format: Option<Format>) -> Result<FileReport, ReadError> {
    let format =
        format
            .or_else(|| Format::of_input(path))
            .ok_or_else(|| ReadError::UnknownFormat {
                path: path.to_path_buf(),
            })?;
    if !format.can_read() {
        return Err(ReadError::NotReadable { format });
    }
    let bytes = std::fs::read(path).map_err(|error| ReadError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    read_bytes(&bytes, format, Some(path))
}

/// Reads a structure file's bytes. `path`, if any, is where they came
/// from: the structures are named after it, as the interface names them;
/// without it they are named after their data blocks.
pub fn read_bytes(
    bytes: &[u8],
    format: Format,
    path: Option<&Path>,
) -> Result<FileReport, ReadError> {
    let report = match format {
        Format::Cif => cif::read(bytes),
        Format::Poscar => poscar::read(bytes),
        Format::Xyz => return Err(ReadError::NotReadable { format }),
    };
    let several = report.structures().count() > 1;
    let file_name = path
        .and_then(Path::file_name)
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let stem = path.map(file_stem);
    let documents = report
        .structures()
        .map(|(block, structure)| {
            let name = match &stem {
                Some(stem) if several => format!("{stem} · {}", block.name),
                Some(stem) => stem.clone(),
                None if block.name.trim().is_empty() => "structure".into(),
                None => block.name.trim().to_string(),
            };
            // The file's own problems belong to each of its structures, as
            // the interface shows them.
            let mut block = block.clone();
            block
                .diagnostics
                .splice(0..0, report.diagnostics.iter().cloned());
            Document::imported(name, &file_name, structure.clone(), Arc::new(block))
        })
        .collect();
    Ok(FileReport {
        format,
        report,
        documents,
    })
}

/// The file's name without extension, with the folder for a bare POSCAR or
/// CONTCAR, as every VASP run names its files: `Si-relax · CONTCAR`.
fn file_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let bare = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
        ["POSCAR", "CONTCAR"]
            .iter()
            .any(|b| n.eq_ignore_ascii_case(b))
    });
    // The interface gets absolute paths; a relative `POSCAR` lies in the
    // working directory.
    let folder = std::path::absolute(path).ok().and_then(|p| {
        p.parent()?
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
    });
    match folder {
        Some(folder) if bare => format!("{folder} · {stem}"),
        _ => stem,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../mw-crystal-io/tests/fixtures")
    }

    #[test]
    fn structures_are_named_as_the_interface_names_them() {
        let rutile = read_file(&fixtures().join("rutile.cif"), None).unwrap();
        assert_eq!(rutile.format, Format::Cif);
        let names: Vec<&str> = rutile.documents.iter().map(Document::name).collect();
        assert_eq!(names, ["rutile"]);

        let multi = read_file(&fixtures().join("multi_block.cif"), None).unwrap();
        let names: Vec<&str> = multi.documents.iter().map(Document::name).collect();
        assert!(names.len() > 1, "{names:?}");
        assert!(
            names.iter().all(|n| n.starts_with("multi_block · ")),
            "{names:?}"
        );

        let contcar = read_file(&fixtures().join("poscar/CONTCAR"), None).unwrap();
        assert_eq!(contcar.format, Format::Poscar);
        assert_eq!(contcar.documents[0].name(), "poscar · CONTCAR");
    }

    #[test]
    fn without_a_path_structures_are_named_after_their_blocks() {
        let text = std::fs::read(fixtures().join("rutile.cif")).unwrap();
        let report = read_bytes(&text, Format::Cif, None).unwrap();
        assert_eq!(report.documents[0].name(), "rutile");
    }

    #[test]
    fn failed_blocks_have_no_document() {
        let report = read_file(&fixtures().join("malformed.cif"), None).unwrap();
        assert!(report.failed_blocks().count() > 0);
        assert!(
            report
                .blocks()
                .all(|(block, document)| document.is_some() == block.structure.is_some())
        );
    }

    #[test]
    fn errors_say_what_is_wrong() {
        let unknown = read_file(Path::new("notes.txt"), None).unwrap_err();
        assert_eq!(unknown.code(), "unknown_format");
        assert!(unknown.to_string().contains("notes.txt"), "{unknown}");
        let xyz = read_file(Path::new("a.xyz"), Some(Format::Xyz)).unwrap_err();
        assert_eq!(xyz.code(), "not_readable");
        let missing = read_file(&fixtures().join("missing.cif"), None).unwrap_err();
        assert_eq!(missing.code(), "unreadable");
        assert!(missing.to_string().contains("missing.cif"), "{missing}");
    }
}
