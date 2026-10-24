//! Undo and redo (ai-integration design §5.2): one stack per document,
//! shared by the user and every tool, so a model can take back its own
//! change and the user can take back the model's.

use mw_crystal::Command;

use crate::registry::{CallError, Effect, Host, Output, Registry, Spec};
use crate::schema::schema_struct;
use crate::structure::{StructureInput, document};

schema_struct! {
    pub struct HistoryOutput {
        /// False when there was nothing to undo or redo.
        pub changed: bool,
        /// The document's revision after the call; it counts changes.
        pub revision: u64,
        /// The kind of command undone or redone, e.g. remove_site; null
        /// when nothing was.
        pub command: Option<String>,
    }
}

/// The command's kind as its JSON names it, e.g. `set_cell`.
fn kind(command: &Command) -> String {
    match command {
        Command::Edit(edit) => serde_json::to_value(edit)
            .ok()
            .and_then(|value| value["command"].as_str().map(str::to_owned))
            .unwrap_or_default(),
        Command::AddMeasurement(_) => "add_measurement".into(),
        Command::RemoveMeasurement(_) => "remove_measurement".into(),
    }
}

pub(crate) fn register(registry: &mut Registry) {
    let spec = |name, description| Spec::new(name, description, Effect::Undoable);

    registry.register(
        spec(
            "history.undo",
            "Undo the last change to a structure, whoever made it: the user in the \
             application or a tool call.",
        ),
        |host: &mut dyn Host, input: StructureInput| {
            let command = document(host, input.structure)?.undo_command().map(kind);
            let changed = host.undo(input.structure)?.is_some();
            let output = HistoryOutput {
                changed,
                revision: document(host, input.structure)?.revision(),
                command,
            };
            let summary = match &output.command {
                Some(kind) => format!("Undid {kind}."),
                None => "Nothing to undo.".into(),
            };
            Ok::<_, CallError>(Output::new(output, summary))
        },
    );

    registry.register(
        spec(
            "history.redo",
            "Redo the last undone change to a structure. A new change after an undo clears \
             what can be redone.",
        ),
        |host: &mut dyn Host, input: StructureInput| {
            let command = document(host, input.structure)?.redo_command().map(kind);
            let changed = host.redo(input.structure)?.is_some();
            let output = HistoryOutput {
                changed,
                revision: document(host, input.structure)?.revision(),
                command,
            };
            let summary = match &output.command {
                Some(kind) => format!("Redid {kind}."),
                None => "Nothing to redo.".into(),
            };
            Ok(Output::new(output, summary))
        },
    );
}
