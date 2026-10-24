//! Who calls and what they may do (ai-integration design §7): the user at
//! the keyboard may do anything; an AI client what the user allowed, and
//! irreversible operations only once the user confirmed them. The registry
//! enforces this whatever a model says ([`crate::Registry::call_as`]), and
//! records every call a client makes for the audit log (design §7.3).

use mw_crystal::Agent;
use mw_project_file::{AuditClient, AuditEffect, AuditRecord, AuditResult, EntryId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::registry::{CallError, CallResult, Effect};

/// Strings in recorded arguments are cut to this many characters.
const MAX_ARGUMENT_CHARS: usize = 200;
/// Recorded arguments larger than this keep only their field names.
const MAX_ARGUMENTS_BYTES: usize = 2048;
/// Recorded summaries and messages are cut to this many characters.
const MAX_RESULT_CHARS: usize = 500;

/// An MCP client, or later the built-in assistant, as it names itself
/// (MCP `clientInfo`). Self-reported: any local process of the user can
/// claim any name, so this tells clients apart for the user's convenience
/// and is no security boundary (design §5.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    /// The identifier, e.g. `claude-code`.
    pub name: String,
    /// The name it shows, e.g. `Claude Code`.
    pub title: Option<String>,
}

impl Client {
    pub fn new(name: impl Into<String>, title: Option<String>) -> Self {
        Self {
            name: name.into(),
            title: title.filter(|t| !t.trim().is_empty()),
        }
    }

    /// The name to show the user.
    pub fn display_name(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }

    /// The client as the author of undo steps.
    pub fn agent(&self) -> Agent {
        Agent::new(self.display_name())
    }

    fn audit(&self) -> AuditClient {
        AuditClient {
            name: self.name.clone(),
            title: self.title.clone(),
        }
    }
}

/// What the user lets AI clients do (design §7.2). Both on by default, as
/// the user decided (2026-10-24): edits can be undone, and every file
/// written is confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    /// Undoable writes: edits, imports, derived structures.
    pub modify_data: bool,
    /// Irreversible writes to files, each still confirmed.
    pub write_files: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            modify_data: true,
            write_files: true,
        }
    }
}

/// Who makes a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The person at the keyboard, through scripts or the command line.
    User,
    Client(Client),
}

/// The circumstances of a call: who makes it, what they may do, and
/// whether the user confirmed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub caller: Caller,
    pub permissions: Permissions,
    /// The user confirmed this call, as irreversible operations need.
    pub confirmed: bool,
}

impl Context {
    /// The user's own call, which may do anything.
    pub fn user() -> Self {
        Self {
            caller: Caller::User,
            permissions: Permissions::default(),
            confirmed: true,
        }
    }

    /// A client's call, with what the user lets it do; not confirmed.
    pub fn client(client: Client, permissions: Permissions) -> Self {
        Self {
            caller: Caller::Client(client),
            permissions,
            confirmed: false,
        }
    }

    /// The same call, confirmed by the user.
    pub fn confirmed(mut self) -> Self {
        self.confirmed = true;
        self
    }

    pub fn client_info(&self) -> Option<&Client> {
        match &self.caller {
            Caller::User => None,
            Caller::Client(client) => Some(client),
        }
    }

    /// Whether the call may run a capability with this effect; the refusal
    /// otherwise, which says what the user can do.
    pub(crate) fn permit(&self, capability: &str, effect: Effect) -> Result<(), CallError> {
        let Caller::Client(client) = &self.caller else {
            return Ok(());
        };
        let who = client.display_name();
        let refused = |what: &str| {
            CallError::new(
                "not_permitted",
                format!(
                    "the user has not allowed {who} to {what}; {capability} was not run. The \
                     user can allow it in Materials Workbench under Settings, MCP clients."
                ),
            )
        };
        match effect {
            Effect::ReadOnly | Effect::View => Ok(()),
            Effect::Undoable if self.permissions.modify_data => Ok(()),
            Effect::Undoable => Err(refused("change data")),
            Effect::External if !self.permissions.write_files => Err(refused("write files")),
            Effect::External if !self.confirmed => Err(CallError::new(
                "not_confirmed",
                format!(
                    "{capability} writes outside the workbench and needs the user's \
                     confirmation, which it did not get; nothing was written"
                ),
            )),
            Effect::External => Ok(()),
            Effect::Instrument => Err(CallError::new(
                "not_permitted",
                format!("{capability} changes an instrument, which AI clients may not do"),
            )),
        }
    }
}

impl Effect {
    pub(crate) fn audit(self) -> AuditEffect {
        match self {
            Self::ReadOnly => AuditEffect::ReadOnly,
            Self::View => AuditEffect::View,
            Self::Undoable => AuditEffect::Undoable,
            Self::External => AuditEffect::External,
            Self::Instrument => AuditEffect::Instrument,
        }
    }
}

/// The record of a client's call.
pub(crate) fn record(
    client: &Client,
    capability: &str,
    structure: Option<EntryId>,
    arguments: &Value,
    effect: Effect,
    confirmed: bool,
    result: Result<&CallResult, &CallError>,
) -> AuditRecord {
    let result = match result {
        Ok(result) => AuditResult::done(cut(&result.summary, MAX_RESULT_CHARS)),
        Err(error) => AuditResult::failed(error.code, cut(&error.message, MAX_RESULT_CHARS)),
    };
    AuditRecord::new(
        Timestamp::now(),
        client.audit(),
        capability,
        structure,
        recorded_arguments(arguments),
        effect.audit(),
        confirmed,
        result,
    )
}

fn cut(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// The arguments as recorded: long strings cut, and only the field names of
/// arguments that are still large.
fn recorded_arguments(arguments: &Value) -> Value {
    fn shorten(value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(cut(text, MAX_ARGUMENT_CHARS)),
            Value::Array(items) => Value::Array(items.iter().map(shorten).collect()),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), shorten(value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    let short = shorten(arguments);
    if short.to_string().len() <= MAX_ARGUMENTS_BYTES {
        return short;
    }
    match &short {
        Value::Object(fields) => Value::Array(
            fields
                .keys()
                .map(|key| Value::String(key.clone()))
                .collect(),
        ),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claude() -> Client {
        Client::new("claude-code", Some("Claude Code".into()))
    }

    #[test]
    fn clients_show_their_title() {
        assert_eq!(claude().display_name(), "Claude Code");
        assert_eq!(claude().agent().name(), "Claude Code");
        let bare = Client::new("inspector", Some(" ".into()));
        assert_eq!(bare.display_name(), "inspector");
    }

    #[test]
    fn effects_need_permissions() {
        let user = Context::user();
        let all = Context::client(claude(), Permissions::default());
        let none = Context::client(
            claude(),
            Permissions {
                modify_data: false,
                write_files: false,
            },
        );
        for effect in [Effect::ReadOnly, Effect::View] {
            assert!(none.permit("x", effect).is_ok());
        }
        assert!(user.permit("x", Effect::External).is_ok());
        assert!(all.permit("x", Effect::Undoable).is_ok());
        assert_eq!(
            none.permit("structure.set_cell", Effect::Undoable)
                .unwrap_err()
                .code,
            "not_permitted"
        );
        // Writing files needs both the permission and the confirmation.
        assert_eq!(
            all.permit("x", Effect::External).unwrap_err().code,
            "not_confirmed"
        );
        assert!(
            all.clone()
                .confirmed()
                .permit("x", Effect::External)
                .is_ok()
        );
        let refused = none.confirmed().permit("x", Effect::External).unwrap_err();
        assert_eq!(refused.code, "not_permitted");
        assert!(refused.message.contains("write files"), "{refused}");
        assert!(user.permit("x", Effect::Instrument).is_ok());
        assert!(all.permit("x", Effect::Instrument).is_err());
    }

    #[test]
    fn recorded_arguments_are_short() {
        let long = "x".repeat(300);
        let recorded = recorded_arguments(&json!({ "path": long, "structure": 1 }));
        assert_eq!(recorded["structure"], 1);
        assert_eq!(recorded["path"].as_str().unwrap().chars().count(), 201);
        let many: Vec<String> = (0..40).map(|_| "y".repeat(150)).collect();
        let recorded = recorded_arguments(&json!({ "labels": many, "structure": 1 }));
        assert_eq!(recorded, json!(["labels", "structure"]));
        assert_eq!(cut("金红石", 2), "金红…");
    }
}
