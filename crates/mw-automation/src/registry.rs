use std::fmt;
use std::sync::Arc;

use mw_crystal::{CrystalDocument, EditError, Guidance};
use mw_crystal_io::ImportedBlock;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::project::Projects;
use crate::schema::Schema;

/// A structure in the open project. The project catalog assigns these, and
/// `mw_project` re-exports this type; it lives here so that capabilities
/// can name structures without the interface crates (design §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StructureEntryId(pub u32);

impl Schema for StructureEntryId {
    fn schema() -> Value {
        json!({ "type": "integer" })
    }
}

/// What calling a capability can change (design §7.1). Clients decide from
/// it whether a call needs the user's confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    ReadOnly,
    /// Changes a document through its undo stack.
    Undoable,
    /// Irreversible, or reaches outside the application: writing files,
    /// sending data elsewhere.
    External,
    /// Changes the state of an instrument (roadmap §13.3).
    Instrument,
}

/// Where a capability can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Only with the application's window, e.g. viewport snapshots.
    Gui,
    /// Only without a window.
    Headless,
    Both,
}

/// A structure a host can hand to capabilities.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct StructureInfo {
    pub id: StructureEntryId,
    pub name: String,
    pub project: String,
}

impl StructureInfo {
    pub fn new(id: StructureEntryId, name: impl Into<String>, project: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            project: project.into(),
        }
    }
}

/// Where capabilities find the documents they work on: the running
/// application, or a session without a window ([`Documents`]).
pub trait Host {
    /// The structures callers can name, in listing order.
    fn structures(&self) -> Vec<StructureInfo>;
    fn document(&self, id: StructureEntryId) -> Option<&CrystalDocument>;
    /// `None` for a document capabilities may read but not edit, as in
    /// the application for now: edits there go through its interface.
    fn document_mut(&mut self, id: StructureEntryId) -> Option<&mut CrystalDocument>;
    /// The data block the structure was read from: its import diagnostics,
    /// and the CIF fields the structure does not hold. `None` for a
    /// structure that came from no file, such as a sample.
    fn source(&self, id: StructureEntryId) -> Option<&ImportedBlock> {
        let _ = id;
        None
    }
    /// Whether the application's window is there.
    fn has_window(&self) -> bool {
        false
    }
    /// The projects the structures belong to, for hosts that have them: the
    /// application, and the MCP server without a window. Documents held on
    /// their own ([`Documents`]) have none; project capabilities are
    /// unavailable there.
    fn projects(&self) -> Option<&dyn Projects> {
        None
    }
    fn projects_mut(&mut self) -> Option<&mut dyn Projects> {
        None
    }
}

/// Documents held in memory, without a window: for tests, and later for the
/// MCP server's headless mode and the command line.
#[derive(Clone, Debug, Default)]
pub struct Documents {
    entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
struct Entry {
    info: StructureInfo,
    document: CrystalDocument,
    source: Option<Arc<ImportedBlock>>,
}

impl Documents {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a document under the next free id.
    pub fn add(
        &mut self,
        name: impl Into<String>,
        project: impl Into<String>,
        document: CrystalDocument,
    ) -> StructureEntryId {
        self.push(name.into(), project.into(), document, None)
    }

    /// Adds a document read from `source`, a file's data block, under the
    /// next free id.
    pub fn add_imported(
        &mut self,
        name: impl Into<String>,
        project: impl Into<String>,
        document: CrystalDocument,
        source: Arc<ImportedBlock>,
    ) -> StructureEntryId {
        self.push(name.into(), project.into(), document, Some(source))
    }

    fn push(
        &mut self,
        name: String,
        project: String,
        document: CrystalDocument,
        source: Option<Arc<ImportedBlock>>,
    ) -> StructureEntryId {
        let next = self.entries.iter().map(|entry| entry.info.id.0).max();
        let id = StructureEntryId(next.map_or(1, |n| n + 1));
        self.entries.push(Entry {
            info: StructureInfo::new(id, name, project),
            document,
            source,
        });
        id
    }

    fn entry(&self, id: StructureEntryId) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.info.id == id)
    }
}

impl Host for Documents {
    fn structures(&self) -> Vec<StructureInfo> {
        self.entries
            .iter()
            .map(|entry| entry.info.clone())
            .collect()
    }

    fn document(&self, id: StructureEntryId) -> Option<&CrystalDocument> {
        self.entry(id).map(|entry| &entry.document)
    }

    fn document_mut(&mut self, id: StructureEntryId) -> Option<&mut CrystalDocument> {
        self.entries
            .iter_mut()
            .find(|entry| entry.info.id == id)
            .map(|entry| &mut entry.document)
    }

    fn source(&self, id: StructureEntryId) -> Option<&ImportedBlock> {
        self.entry(id)?.source.as_deref()
    }
}

/// A successful call: structured output that matches the capability's
/// output schema, and a short English summary for a model to read first
/// (design §4).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct CallResult {
    pub output: Value,
    pub summary: String,
}

impl CallResult {
    /// For entry points that add operations of their own next to the
    /// registry's, such as opening a file in the MCP server.
    pub fn new(output: Value, summary: impl Into<String>) -> Self {
        Self {
            output,
            summary: summary.into(),
        }
    }
}

/// A failed call. Callers can recover: the message says what was wrong in
/// English, and the guidance what to do instead (design §4, crystallography
/// design §7.1). Nothing was changed.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct CallError {
    /// Stable, e.g. `unknown_structure` or one of [`EditError::code`].
    pub code: &'static str,
    pub message: String,
    pub guidance: Vec<Guidance>,
}

impl CallError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            guidance: Vec::new(),
        }
    }

    /// `{"code", "message", "guidance"}`, for a tool result marked as an
    /// error.
    pub fn to_json(&self) -> Value {
        json!({
            "code": self.code,
            "message": self.message,
            "guidance": self.guidance,
        })
    }
}

impl From<EditError> for CallError {
    fn from(error: EditError) -> Self {
        Self {
            code: error.code(),
            message: error.to_string(),
            guidance: error.guidance(),
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CallError {}

/// What a handler returns.
pub struct Output<O> {
    pub value: O,
    pub summary: String,
}

impl<O> Output<O> {
    pub fn new(value: O, summary: impl Into<String>) -> Self {
        Self {
            value,
            summary: summary.into(),
        }
    }
}

/// The declaration of a capability, apart from its schemas and handler.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// Stable, dotted, e.g. `structure.set_cell`.
    pub name: &'static str,
    /// English, for models: what it does, when to use it, units, limits.
    pub description: &'static str,
    pub effect: Effect,
    pub availability: Availability,
}

type Handler = Box<dyn Fn(&mut dyn Host, Value) -> Result<CallResult, CallError> + Send + Sync>;

/// One operation that every entry point can call: the interface, scripts,
/// the command line, MCP clients and the built-in assistant (design §4).
pub struct Capability {
    pub name: &'static str,
    pub description: &'static str,
    pub effect: Effect,
    pub availability: Availability,
    pub input_schema: Value,
    pub output_schema: Value,
    handler: Handler,
}

impl fmt::Debug for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Capability")
            .field("name", &self.name)
            .field("effect", &self.effect)
            .field("availability", &self.availability)
            .finish_non_exhaustive()
    }
}

/// The capabilities, by name.
#[derive(Debug, Default)]
pub struct Registry {
    capabilities: Vec<Capability>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every capability this crate provides.
    pub fn standard() -> Self {
        let mut registry = Self::new();
        crate::project::register(&mut registry);
        crate::structure::register(&mut registry);
        crate::files::register(&mut registry);
        crate::history::register(&mut registry);
        registry
    }

    /// Adds a capability whose input and output schemas come from `I` and
    /// `O`. Panics if the name is taken.
    pub fn register<I, O, F>(&mut self, spec: Spec, handler: F)
    where
        I: Schema + DeserializeOwned,
        O: Schema + Serialize,
        F: Fn(&mut dyn Host, I) -> Result<Output<O>, CallError> + Send + Sync + 'static,
    {
        assert!(
            self.get(spec.name).is_none(),
            "capability {} registered twice",
            spec.name
        );
        let handler = move |host: &mut dyn Host, input: Value| {
            let input: I = serde_json::from_value(input)
                .map_err(|error| CallError::new("invalid_arguments", error.to_string()))?;
            let output = handler(host, input)?;
            Ok(CallResult {
                output: serde_json::to_value(output.value).expect("outputs serialize"),
                summary: output.summary,
            })
        };
        self.capabilities.push(Capability {
            name: spec.name,
            description: spec.description,
            effect: spec.effect,
            availability: spec.availability,
            input_schema: I::schema(),
            output_schema: O::schema(),
            handler: Box::new(handler),
        });
    }

    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    pub fn get(&self, name: &str) -> Option<&Capability> {
        self.capabilities.iter().find(|c| c.name == name)
    }

    /// Calls a capability with JSON arguments; `null` counts as `{}`.
    pub fn call(
        &self,
        host: &mut dyn Host,
        name: &str,
        input: Value,
    ) -> Result<CallResult, CallError> {
        let capability = self.get(name).ok_or_else(|| {
            CallError::new("unknown_capability", format!("no capability named {name}"))
        })?;
        let available = match capability.availability {
            Availability::Gui => host.has_window(),
            Availability::Headless => !host.has_window(),
            Availability::Both => true,
        };
        if !available {
            return Err(CallError::new(
                "unavailable",
                format!("{name} is not available in this mode"),
            ));
        }
        let input = if input.is_null() { json!({}) } else { input };
        (capability.handler)(host, input)
    }
}
