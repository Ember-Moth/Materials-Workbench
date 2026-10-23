//! The structures a client opened in this process, and opening them
//! (`file.open`). Until project files exist (M6) the server holds one
//! in-memory project, `session`; structure ids are stable for the life of
//! the process.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mw_automation::schema::{Schema, object};
use mw_automation::{
    CallError, CallResult, Documents, Registry, StructureEntryId, diagnostic_message,
};
use mw_crystal::CrystalDocument;
use mw_crystal_io::{Diagnostic, ImportedBlock, Severity, cif};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::access::Access;

/// The project segment of resource URIs while the server holds no project
/// files (design §5.2).
/// Mock: one in-memory project stands in for project files (M6).
pub const PROJECT: &str = "session";
/// The project name `structure.list` shows.
const PROJECT_NAME: &str = "Opened files";
/// Errors a refused file lists at most.
const MAX_ERRORS: usize = 20;

/// The URI of a structure's CIF text (design §5.2).
pub fn resource_uri(id: StructureEntryId) -> String {
    format!("mw://project/{PROJECT}/structure/{}.cif", id.0)
}

/// The structure a resource URI names, if it is one of ours.
pub fn parse_resource_uri(uri: &str) -> Option<StructureEntryId> {
    let rest = uri.strip_prefix("mw://project/")?;
    let (project, rest) = rest.split_once('/')?;
    let id = rest.strip_prefix("structure/")?.strip_suffix(".cif")?;
    if project != PROJECT || id.starts_with('+') {
        return None;
    }
    id.parse().ok().map(StructureEntryId)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenInput {
    pub path: String,
}

impl Schema for OpenInput {
    fn schema() -> Value {
        object(&[(
            "path",
            String::schema(),
            "Path of a .cif file, absolute or relative to the server's working directory. Only \
             files inside the directories the user started the server with can be opened.",
        )])
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct OpenOutput {
    /// Canonical path.
    file: String,
    structures: Vec<OpenedStructure>,
    skipped_blocks: Vec<SkippedBlock>,
}

#[derive(Clone, Debug, Serialize)]
struct OpenedStructure {
    structure: StructureEntryId,
    name: String,
    block: String,
    resource: String,
    warnings: usize,
    infos: usize,
}

#[derive(Clone, Debug, Serialize)]
struct SkippedBlock {
    block: String,
    line: u32,
    errors: Vec<String>,
}

impl Schema for OpenOutput {
    fn schema() -> Value {
        let structure = object(&[
            (
                "structure",
                StructureEntryId::schema(),
                "Id to name the structure by in other tools.",
            ),
            (
                "name",
                String::schema(),
                "The file name without extension, plus the data block when the file has \
                 several. Quoted from the file.",
            ),
            (
                "block",
                String::schema(),
                "The data block, as the file names it.",
            ),
            (
                "resource",
                String::schema(),
                "URI of the structure's CIF text as a resource.",
            ),
            (
                "warnings",
                usize::schema(),
                "Import warnings; structure_diagnostics lists them.",
            ),
            (
                "infos",
                usize::schema(),
                "Changes the importer made, e.g. sites moved onto special positions.",
            ),
        ]);
        let skipped = object(&[
            ("block", String::schema(), ""),
            ("line", u32::schema(), "Line of the block's data_ header."),
            (
                "errors",
                json!({ "type": "array", "items": { "type": "string" } }),
                "Why the block yields no structure, with source lines. Quoted from the file.",
            ),
        ]);
        object(&[
            ("file", String::schema(), "The file's canonical path."),
            (
                "structures",
                json!({ "type": "array", "items": structure }),
                "The structures read from the file, in file order.",
            ),
            (
                "skipped_blocks",
                json!({ "type": "array", "items": skipped }),
                "Data blocks with errors, which yield no structure.",
            ),
        ])
    }
}

/// A file read and parsed, its structures analyzed, not yet in the
/// session. Built off the async runtime: symmetry analysis takes time.
pub struct ReadFile {
    path: PathBuf,
    digest: (u64, usize),
    structures: Vec<(String, Arc<ImportedBlock>, CrystalDocument)>,
    skipped: Vec<SkippedBlock>,
    /// Problems of the whole file, for a file without structures.
    file_errors: Vec<String>,
}

impl ReadFile {
    pub fn read(access: &Access, path: &str) -> Result<Self, CallError> {
        let (path, bytes) = access.read(path)?;
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let digest = (hasher.finish(), bytes.len());
        let report = cif::read(&bytes);
        let stem = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let several = report.structures().count() > 1;
        let structures = report
            .structures()
            .map(|(block, structure)| {
                // Named and with the file's diagnostics, as the interface
                // opens files (mw-structure/src/import.rs).
                let name = if several {
                    format!("{stem} · {}", block.name)
                } else {
                    stem.clone()
                };
                let mut block = block.clone();
                block
                    .diagnostics
                    .splice(0..0, report.diagnostics.iter().cloned());
                let document = CrystalDocument::new(structure.clone());
                (name, Arc::new(block), document)
            })
            .collect();
        let skipped = report
            .failed_blocks()
            .map(|block| SkippedBlock {
                block: block.name.clone(),
                line: block.line,
                errors: errors(&block.diagnostics),
            })
            .collect();
        Ok(Self {
            path,
            digest,
            structures,
            skipped,
            file_errors: errors(&report.diagnostics),
        })
    }
}

/// Error messages with their lines, for a model to read.
fn errors(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .take(MAX_ERRORS)
        .map(|d| match d.line() {
            Some(line) => format!("line {line}: {}", diagnostic_message(d)),
            None => diagnostic_message(d),
        })
        .collect()
}

struct OpenedFile {
    path: PathBuf,
    digest: (u64, usize),
    output: OpenOutput,
}

/// The registry, the open structures and the files they came from.
pub struct Session {
    registry: Registry,
    documents: Documents,
    files: Vec<OpenedFile>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            registry: Registry::standard(),
            documents: Documents::new(),
            files: Vec::new(),
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn documents(&self) -> &Documents {
        &self.documents
    }

    pub fn call(&mut self, capability: &str, input: Value) -> Result<CallResult, CallError> {
        self.registry.call(&mut self.documents, capability, input)
    }

    /// Adds a file's structures. A file opened before with the same
    /// contents gives the same structures again, so opening is idempotent.
    /// Returns whether structures were added.
    pub fn add(&mut self, file: ReadFile) -> Result<(CallResult, bool), CallError> {
        if let Some(opened) = self
            .files
            .iter()
            .find(|f| f.path == file.path && f.digest == file.digest)
        {
            let summary = format!("Already open. {}", summary(&opened.output));
            return Ok((result(&opened.output, summary), false));
        }
        if file.structures.is_empty() {
            let mut errors = file.file_errors;
            errors.extend(file.skipped.into_iter().flat_map(|block| block.errors));
            errors.truncate(MAX_ERRORS);
            let message = if errors.is_empty() {
                format!("{} holds no structure", display(&file.path))
            } else {
                format!(
                    "{} holds no structure the importer could read: {}",
                    display(&file.path),
                    errors.join("; ")
                )
            };
            return Err(CallError::new("no_structure", message));
        }
        let structures = file
            .structures
            .into_iter()
            .map(|(name, block, document)| {
                let count = |s| {
                    block
                        .diagnostics
                        .iter()
                        .filter(|d| d.severity() == s)
                        .count()
                };
                let (warnings, infos) = (count(Severity::Warning), count(Severity::Info));
                let block_name = block.name.clone();
                let id = self
                    .documents
                    .add_imported(name.clone(), PROJECT_NAME, document, block);
                OpenedStructure {
                    structure: id,
                    name,
                    block: block_name,
                    resource: resource_uri(id),
                    warnings,
                    infos,
                }
            })
            .collect();
        let output = OpenOutput {
            file: display(&file.path),
            structures,
            skipped_blocks: file.skipped,
        };
        let summary = summary(&output);
        self.files.push(OpenedFile {
            path: file.path,
            digest: file.digest,
            output: output.clone(),
        });
        Ok((result(&output, summary), true))
    }
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn result(output: &OpenOutput, summary: String) -> CallResult {
    CallResult::new(
        serde_json::to_value(output).expect("outputs serialize"),
        summary,
    )
}

fn summary(output: &OpenOutput) -> String {
    let structures = output
        .structures
        .iter()
        .map(|s| {
            format!(
                "{} (id {}, {} warnings, {} importer changes)",
                s.name, s.structure.0, s.warnings, s.infos
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let skipped = if output.skipped_blocks.is_empty() {
        String::new()
    } else {
        format!(
            " Skipped {} blocks with errors: {}.",
            output.skipped_blocks.len(),
            output
                .skipped_blocks
                .iter()
                .map(|b| b.block.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!("Opened {}: {structures}.{skipped}", output.file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_uris_round_trip() {
        let id = StructureEntryId(12);
        assert_eq!(resource_uri(id), "mw://project/session/structure/12.cif");
        assert_eq!(parse_resource_uri(&resource_uri(id)), Some(id));
        for other in [
            "mw://project/1/structure/12.cif",
            "mw://project/session/structure/12.poscar",
            "mw://project/session/structure/+12.cif",
            "mw://project/session/structure/x.cif",
            "file:///tmp/12.cif",
        ] {
            assert_eq!(parse_resource_uri(other), None, "{other}");
        }
    }
}
