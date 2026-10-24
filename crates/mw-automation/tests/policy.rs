//! What AI clients may do (ai-integration design §7.2): edits only with the
//! user's permission, files written only with the permission and the
//! user's confirmation of that very call, whatever the arguments; the
//! user's own calls need neither. Every client call is recorded.

use std::path::{Path, PathBuf};

use mw_automation::{
    CallError, Client, Context, Documents, Host, Permissions, Registry, StructureEntryId,
    StructureInfo,
};
use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{Agent, Command, CrystalDocument, Outcome};
use mw_project_file::{AuditEffect, AuditRecord, ProjectId};
use serde_json::{Value, json};

/// Documents, and one directory files may be written to.
struct Desk {
    documents: Documents,
    dir: PathBuf,
}

impl Host for Desk {
    fn structures(&self) -> Vec<StructureInfo> {
        self.documents.structures()
    }
    fn document(&self, id: StructureEntryId) -> Option<&CrystalDocument> {
        self.documents.document(id)
    }
    fn execute(
        &mut self,
        id: StructureEntryId,
        command: Command,
        agent: Option<Agent>,
    ) -> Result<Outcome, CallError> {
        self.documents.execute(id, command, agent)
    }
    fn undo(&mut self, id: StructureEntryId) -> Result<Option<Outcome>, CallError> {
        self.documents.undo(id)
    }
    fn redo(&mut self, id: StructureEntryId) -> Result<Option<Outcome>, CallError> {
        self.documents.redo(id)
    }
    fn audit(&mut self, project: Option<ProjectId>, record: AuditRecord) {
        self.documents.audit(project, record);
    }
    fn writable(&self, path: &str) -> Result<PathBuf, CallError> {
        let path = Path::new(path);
        if path.parent() == Some(self.dir.as_path()) {
            Ok(path.to_path_buf())
        } else {
            Err(CallError::new("not_allowed", "outside"))
        }
    }
}

fn desk() -> (tempfile::TempDir, Desk, StructureEntryId) {
    let temp = tempfile::tempdir().unwrap();
    let mut documents = Documents::new();
    let sample = sample::load(SampleKey::Rutile);
    let id = documents.add(
        "Rutile",
        "Samples",
        CrystalDocument::with_derived(sample.structure, sample.derived),
    );
    let dir = temp.path().canonicalize().unwrap();
    (temp, Desk { documents, dir }, id)
}

fn claude(permissions: Permissions) -> Context {
    Context::client(
        Client::new("claude-code", Some("Claude Code".into())),
        permissions,
    )
}

const NOTHING: Permissions = Permissions {
    modify_data: false,
    write_files: false,
};

fn last_record(desk: &Desk) -> &AuditRecord {
    desk.documents.audit_log().last().expect("a record")
}

#[test]
fn edits_need_the_permission() {
    let (_temp, mut desk, id) = desk();
    let registry = Registry::standard();
    let edit = json!({ "structure": id, "values": [{ "parameter": "a", "value": 4.7 }] });

    let refused = registry.call_as(
        &mut desk,
        &claude(NOTHING),
        "structure.set_cell",
        edit.clone(),
    );
    let refused = refused.unwrap_err();
    assert_eq!(refused.code, "not_permitted");
    assert!(refused.message.contains("Claude Code"), "{refused}");
    assert_eq!(desk.document(id).unwrap().revision(), 0);
    let record = last_record(&desk);
    assert_eq!(record.capability, "structure.set_cell");
    assert_eq!(record.effect, AuditEffect::Undoable);
    assert_eq!(record.result.code.as_deref(), Some("not_permitted"));
    // Queries need nothing.
    let summary = json!({ "structure": id });
    assert!(
        registry
            .call_as(&mut desk, &claude(NOTHING), "structure.summary", summary)
            .is_ok()
    );

    let allowed = claude(Permissions::default());
    let done = registry.call_as(&mut desk, &allowed, "structure.set_cell", edit);
    assert!(done.unwrap().output["changed"].as_bool().unwrap());
    let document = desk.document(id).unwrap();
    assert_eq!(document.undo_agent().map(Agent::name), Some("Claude Code"));
    // Undoing is a write too.
    let undo = json!({ "structure": id });
    let refused = registry.call_as(&mut desk, &claude(NOTHING), "history.undo", undo.clone());
    assert_eq!(refused.unwrap_err().code, "not_permitted");
    registry
        .call_as(&mut desk, &allowed, "history.undo", undo)
        .unwrap();
    assert_eq!(desk.document(id).unwrap().undo_command(), None);
    // The user's own calls are not recorded.
    let count = desk.documents.audit_log().len();
    registry
        .call(&mut desk, "structure.summary", json!({ "structure": id }))
        .unwrap();
    assert_eq!(desk.documents.audit_log().len(), count);
}

#[test]
fn files_are_written_only_once_confirmed() {
    let (_temp, mut desk, id) = desk();
    let registry = Registry::standard();
    let path = desk.dir.join("POSCAR");
    let input = |overwrite: bool| -> Value {
        json!({
            "structure": id,
            "format": "poscar",
            "path": path.to_str().unwrap(),
            "overwrite": overwrite,
        })
    };
    let allowed = claude(Permissions::default());

    // What the user is asked, before anything happens.
    let question = registry
        .confirmation(&desk, "structure.export_file", input(false))
        .unwrap()
        .unwrap();
    assert!(question.starts_with("Write Rutile as POSCAR"), "{question}");
    assert!(question.contains(path.to_str().unwrap()), "{question}");
    assert!(!path.exists());
    // Other capabilities need no confirmation.
    let none = registry.confirmation(&desk, "structure.summary", json!({ "structure": id }));
    assert_eq!(none.unwrap(), None);

    // A model cannot skip the question.
    let refused = registry.call_as(&mut desk, &allowed, "structure.export_file", input(false));
    assert_eq!(refused.unwrap_err().code, "not_confirmed");
    assert!(!path.exists());
    let record = last_record(&desk);
    assert_eq!(record.effect, AuditEffect::External);
    assert!(!record.confirmed);

    let confirmed = allowed.clone().confirmed();
    let written = registry
        .call_as(&mut desk, &confirmed, "structure.export_file", input(false))
        .unwrap();
    assert_eq!(written.output["replaced"], false);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("Rutile · Samples"), "{text}");
    let record = last_record(&desk);
    assert!(record.confirmed && record.result.ok);

    // An existing file stays unless the call says to replace it.
    let exists = registry.confirmation(&desk, "structure.export_file", input(false));
    assert_eq!(exists.unwrap_err().code, "file_exists");
    let question = registry
        .confirmation(&desk, "structure.export_file", input(true))
        .unwrap()
        .unwrap();
    assert!(
        question.ends_with("replacing the file there."),
        "{question}"
    );
    let replaced = registry
        .call_as(&mut desk, &confirmed, "structure.export_file", input(true))
        .unwrap();
    assert_eq!(replaced.output["replaced"], true);

    // Confirmed but not allowed, or outside the allowed places.
    let forbidden = claude(Permissions {
        modify_data: true,
        write_files: false,
    })
    .confirmed();
    let refused = registry.call_as(&mut desk, &forbidden, "structure.export_file", input(true));
    assert_eq!(refused.unwrap_err().code, "not_permitted");
    let outside = json!({
        "structure": id,
        "format": "cif",
        "path": "/elsewhere/x.cif",
        "overwrite": false,
    });
    let refused = registry.confirmation(&desk, "structure.export_file", outside.clone());
    assert_eq!(refused.unwrap_err().code, "not_allowed");
    let refused = registry.call_as(&mut desk, &confirmed, "structure.export_file", outside);
    assert_eq!(refused.unwrap_err().code, "not_allowed");

    // The user's own call writes without asking.
    std::fs::remove_file(&path).unwrap();
    registry
        .call(&mut desk, "structure.export_file", input(false))
        .unwrap();
    assert!(path.exists());
}
