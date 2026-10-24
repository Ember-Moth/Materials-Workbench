//! What reading a file found, for people and programs: each diagnostic as
//! `structure.diagnostics` gives it, with the registry's English message
//! (`mw_automation::diagnostic_message`).

use mw_automation::diagnostic_message;
use mw_crystal_io::{Diagnostic, ImportedBlock, Severity};
use serde_json::{Value, json};

use crate::document::Document;
use crate::read::FileReport;

/// `error`, `warning` or `info`.
pub fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

/// A diagnostic as `structure.diagnostics` gives it: severity, code, line
/// and English message.
pub fn diagnostic_json(diagnostic: &Diagnostic) -> Value {
    json!({
        "severity": severity_name(diagnostic.severity()),
        "code": diagnostic.code(),
        "line": diagnostic.line(),
        "message": diagnostic_message(diagnostic),
    })
}

/// E.g. `line 12: Tag '_cell_length_a' has no value. (syntax.missing_value)`.
pub fn diagnostic_line(diagnostic: &Diagnostic) -> String {
    let line = diagnostic
        .line()
        .map_or_else(|| "file".to_string(), |line| format!("line {line}"));
    format!(
        "{line}: {} ({})",
        diagnostic_message(diagnostic),
        diagnostic.code()
    )
}

/// Why a block was not imported: `block "broken" (line 2) was not
/// imported: 6 errors`, and a line for each of its first `max` errors
/// ([`diagnostic_line`]).
pub fn failure_text(block: &ImportedBlock, max: usize) -> (String, Vec<String>) {
    let errors: Vec<&Diagnostic> = block
        .diagnostics
        .iter()
        .filter(|d| d.severity() == Severity::Error)
        .collect();
    let heading = format!(
        "block {:?} (line {}) was not imported: {} error{}",
        block.name,
        block.line,
        errors.len(),
        if errors.len() == 1 { "" } else { "s" }
    );
    let mut lines: Vec<String> = errors
        .iter()
        .take(max)
        .map(|d| diagnostic_line(d))
        .collect();
    if errors.len() > max {
        lines.push(format!("… {} more", errors.len() - max));
    }
    (heading, lines)
}

fn block_json(block: &ImportedBlock, document: Option<&Document>) -> Value {
    let count = |severity| {
        block
            .diagnostics
            .iter()
            .filter(|d| d.severity() == severity)
            .count()
    };
    json!({
        "block": block.name,
        "line": block.line,
        "structure": document.map(Document::name),
        "errors": count(Severity::Error),
        "warnings": count(Severity::Warning),
        "infos": count(Severity::Info),
        "diagnostics": block.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>(),
    })
}

impl FileReport {
    /// Whether the file failed: a block could not be imported, or it holds
    /// no structure at all.
    pub fn failed(&self) -> bool {
        self.failed_blocks().next().is_some() || self.documents.is_empty()
    }

    /// Everything reading the file found, as JSON: the format, how many
    /// structures were imported, whether it failed, the problems that
    /// belong to no block, and each data block (failed ones too) with the
    /// structure made from it and its own diagnostics.
    pub fn diagnostics_json(&self) -> Value {
        json!({
            "format": self.format.name(),
            "imported": self.documents.len(),
            "failed": self.failed(),
            "diagnostics": self.report.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>(),
            "blocks": self
                .blocks()
                .map(|(block, document)| block_json(block, document))
                .collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use mw_automation::{Documents, Registry};
    use mw_crystal::CrystalDocument;

    use super::*;
    use crate::read::read_file;

    fn read(name: &str) -> FileReport {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mw-crystal-io/tests/fixtures")
            .join(name);
        read_file(&path, None).unwrap()
    }

    #[test]
    fn diagnostics_are_structure_diagnostics() {
        for name in [
            "rutile_near_special.cif",
            "zincite_legacy.cif",
            "poscar/CONTCAR",
        ] {
            let report = read(name);
            let document = &report.documents[0];
            let mut host = Documents::new();
            let id = host.add_imported(
                document.name(),
                document.file(),
                CrystalDocument::new(document.structure().clone()),
                Arc::new(document.source().unwrap().clone()),
            );
            let input = serde_json::json!({ "structure": id, "offset": 0, "limit": 500 });
            let expected = Registry::standard()
                .call(&mut host, "structure.diagnostics", input)
                .unwrap();
            let json = report.diagnostics_json();
            let mut listed = json["diagnostics"].as_array().unwrap().clone();
            listed.extend(json["blocks"][0]["diagnostics"].as_array().unwrap().clone());
            assert_eq!(
                Value::Array(listed),
                expected.output["diagnostics"],
                "{name}"
            );
            assert_eq!(json["failed"], false);
        }
    }

    #[test]
    fn failed_files_say_so() {
        let report = read("malformed.cif");
        let json = report.diagnostics_json();
        assert_eq!(json["failed"], true);
        assert_eq!(json["blocks"][0]["structure"], Value::Null);
        assert!(json["blocks"][0]["errors"].as_u64().unwrap() > 0);
        let error = report.failed_blocks().next().unwrap().diagnostics[0].clone();
        assert!(diagnostic_line(&error).starts_with("line "));
    }
}
