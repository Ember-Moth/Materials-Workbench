//! The audit log of AI operations, `audit.jsonl` (ai-integration design
//! §7.3, design §15): one JSON line per call an MCP client or the assistant
//! made on the project, appended and never rewritten.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EntryId, Timestamp};

/// The log's file name inside a project directory.
pub const AUDIT: &str = "audit.jsonl";

/// An AI client, as it names itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditClient {
    /// The client's identifier, e.g. `claude-code`.
    pub name: String,
    /// The name it shows, e.g. `Claude Code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// What a capability can change (ai-integration design §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEffect {
    ReadOnly,
    View,
    Undoable,
    External,
    Instrument,
}

/// How a call ended: a summary, or an error's code and message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl AuditResult {
    pub fn done(summary: impl Into<String>) -> Self {
        Self {
            ok: true,
            summary: Some(summary.into()),
            code: None,
            message: None,
        }
    }

    pub fn failed(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            summary: None,
            code: Some(code.into()),
            message: Some(message.into()),
        }
    }
}

/// One call, as a line of the log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AuditRecord {
    pub time: Timestamp,
    pub client: AuditClient,
    /// The capability's registry name, e.g. `structure.set_cell`.
    pub capability: String,
    /// The entry the call was about, when it named a structure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<EntryId>,
    /// The call's arguments, long strings cut.
    pub arguments: Value,
    pub effect: AuditEffect,
    /// Whether the user confirmed the call.
    pub confirmed: bool,
    pub result: AuditResult,
}

impl AuditRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        time: Timestamp,
        client: AuditClient,
        capability: impl Into<String>,
        structure: Option<EntryId>,
        arguments: Value,
        effect: AuditEffect,
        confirmed: bool,
        result: AuditResult,
    ) -> Self {
        Self {
            time,
            client,
            capability: capability.into(),
            structure,
            arguments,
            effect,
            confirmed,
            result,
        }
    }

    /// The record as one line of the log, without its line break.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("audit records serialize")
    }
}

/// Appends records to the log in the project directory `dir`, creating it.
/// The records go out in one write, then the file is synced.
pub fn append_audit(dir: &Path, records: &[AuditRecord]) -> io::Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let mut text = String::new();
    for record in records {
        text.push_str(&record.to_line());
        text.push('\n');
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(AUDIT))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// The log in the project directory `dir`, oldest first; empty if there is
/// none. Lines that do not read as records are skipped, so that later
/// versions can add to them.
pub fn read_audit(dir: &Path) -> io::Result<Vec<AuditRecord>> {
    let text = match fs::read_to_string(dir.join(AUDIT)) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(capability: &str, result: AuditResult) -> AuditRecord {
        AuditRecord::new(
            Timestamp::from_unix(1_792_657_700),
            AuditClient {
                name: "claude-code".into(),
                title: Some("Claude Code".into()),
            },
            capability,
            None,
            json!({ "structure": 1 }),
            AuditEffect::Undoable,
            false,
            result,
        )
    }

    #[test]
    fn records_are_lines_of_json() {
        let done = record(
            "structure.set_cell",
            AuditResult::done("The cell is now 5 Å."),
        );
        assert_eq!(
            done.to_line(),
            r#"{"time":"2026-10-22T08:28:20Z","client":{"name":"claude-code","title":"Claude Code"},"capability":"structure.set_cell","arguments":{"structure":1},"effect":"undoable","confirmed":false,"result":{"ok":true,"summary":"The cell is now 5 Å."}}"#
        );
        let failed = record(
            "structure.set_cell",
            AuditResult::failed("not_permitted", "not allowed"),
        );
        assert!(
            failed.to_line().contains(
                r#""result":{"ok":false,"code":"not_permitted","message":"not allowed"}"#
            )
        );
        let read: AuditRecord = serde_json::from_str(&failed.to_line()).unwrap();
        assert_eq!(read, failed);
    }

    #[test]
    fn the_log_is_appended_and_unknown_lines_skipped() {
        let temp = tempfile::tempdir().unwrap();
        assert!(read_audit(temp.path()).unwrap().is_empty());
        let first = record("structure.summary", AuditResult::done("TiO2"));
        let second = record("history.undo", AuditResult::done("Undid set_cell."));
        append_audit(temp.path(), std::slice::from_ref(&first)).unwrap();
        // A line from a later version, or one cut short by a crash.
        let mut file = OpenOptions::new()
            .append(true)
            .open(temp.path().join(AUDIT))
            .unwrap();
        file.write_all(b"{\"time\": 3}\n").unwrap();
        append_audit(temp.path(), std::slice::from_ref(&second)).unwrap();
        append_audit(temp.path(), &[]).unwrap();
        assert_eq!(read_audit(temp.path()).unwrap(), [first, second]);
    }
}
