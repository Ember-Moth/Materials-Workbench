use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use mw_crystal::{Agent, Command, CrystalDocument, EditError, Guidance, Outcome};
use mw_crystal_io::ImportedBlock;
use mw_project_file::{AuditRecord, EntryId, ProjectId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::policy::{self, Client, Context, Permissions};
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
/// it whether a call needs the user's confirmation; the registry decides
/// from it what a client may run (design §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    ReadOnly,
    /// Changes only what the window shows: the selection, the display
    /// model, the camera. No data, no undo step.
    View,
    /// Changes a document through its undo stack, or adds a structure to a
    /// project.
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
/// application, the MCP server reading project files, or documents held in
/// memory ([`Documents`]).
pub trait Host {
    /// The structures callers can name, in listing order.
    fn structures(&self) -> Vec<StructureInfo>;
    fn document(&self, id: StructureEntryId) -> Option<&CrystalDocument>;
    /// Runs a command on a structure as one undo step made by `agent`
    /// (`None` for the user), the way the host shows and keeps its edits:
    /// the application through its workspace, the server without a window
    /// saving the project afterwards (design §5.3). Only called for a
    /// structure [`Self::document`] has.
    fn execute(
        &mut self,
        id: StructureEntryId,
        command: Command,
        agent: Option<Agent>,
    ) -> Result<Outcome, CallError>;
    /// Undoes the structure's last step, whoever made it; `None` when there
    /// was none.
    fn undo(&mut self, id: StructureEntryId) -> Result<Option<Outcome>, CallError>;
    /// Redoes the structure's last undone step; `None` when there was none.
    fn redo(&mut self, id: StructureEntryId) -> Result<Option<Outcome>, CallError>;
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
    /// The client making the current call, whose edits become its undo
    /// steps. The registry tells hosts during each call
    /// ([`Registry::call_as`]); hosts need not implement it.
    fn agent(&self) -> Option<Agent> {
        None
    }
    /// Keeps the record of a client's call for the audit log of `project`
    /// (design §7.3). Hosts without projects drop it.
    fn audit(&mut self, project: Option<ProjectId>, record: AuditRecord) {
        let _ = (project, record);
    }
    /// The canonical path of a file a client asked to write, if it may:
    /// inside the directories the user allowed or an open project's
    /// directory (design §7.2).
    fn writable(&self, path: &str) -> Result<PathBuf, CallError> {
        let _ = path;
        Err(CallError::new(
            "unavailable",
            "writing files is not available in this mode",
        ))
    }
    /// What the user lets `client` do, as the settings say now (design
    /// §7.2).
    fn permissions(&self, client: &Client) -> Permissions {
        let _ = client;
        Permissions::default()
    }
    /// Asks the user in the application whether `client` may make the call
    /// `question` describes; the answer comes later, through `answer`.
    /// `false` when this host cannot ask, as without a window: then MCP's
    /// elicitation asks instead (design §5.3).
    fn ask(&mut self, client: &Client, question: &str, answer: Answer) -> bool {
        let _ = (client, question, answer);
        false
    }
}

/// Takes the user's answer to [`Host::ask`]: `true` to go ahead.
pub type Answer = Box<dyn FnOnce(bool) + Send>;

/// Documents held in memory, without a window: for tests, and later for the
/// command line. Records of clients' calls are kept ([`Self::audit_log`]).
#[derive(Clone, Debug, Default)]
pub struct Documents {
    entries: Vec<Entry>,
    audit: Vec<AuditRecord>,
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

    /// The records of clients' calls, oldest first.
    pub fn audit_log(&self) -> &[AuditRecord] {
        &self.audit
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

    fn document_mut(&mut self, id: StructureEntryId) -> Result<&mut CrystalDocument, CallError> {
        self.entries
            .iter_mut()
            .find(|entry| entry.info.id == id)
            .map(|entry| &mut entry.document)
            .ok_or_else(|| crate::project::unknown_structure(id))
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

    fn execute(
        &mut self,
        id: StructureEntryId,
        command: Command,
        agent: Option<Agent>,
    ) -> Result<Outcome, CallError> {
        Ok(self.document_mut(id)?.execute_by(command, agent)?)
    }

    fn undo(&mut self, id: StructureEntryId) -> Result<Option<Outcome>, CallError> {
        Ok(self.document_mut(id)?.undo())
    }

    fn redo(&mut self, id: StructureEntryId) -> Result<Option<Outcome>, CallError> {
        Ok(self.document_mut(id)?.redo())
    }

    fn source(&self, id: StructureEntryId) -> Option<&ImportedBlock> {
        self.entry(id)?.source.as_deref()
    }

    fn audit(&mut self, _project: Option<ProjectId>, record: AuditRecord) {
        self.audit.push(record);
    }
}

/// The host during one call, which knows the caller's agent.
struct Calling<'a> {
    host: &'a mut dyn Host,
    agent: Option<Agent>,
}

impl Host for Calling<'_> {
    fn structures(&self) -> Vec<StructureInfo> {
        self.host.structures()
    }
    fn document(&self, id: StructureEntryId) -> Option<&CrystalDocument> {
        self.host.document(id)
    }
    fn execute(
        &mut self,
        id: StructureEntryId,
        command: Command,
        agent: Option<Agent>,
    ) -> Result<Outcome, CallError> {
        self.host.execute(id, command, agent)
    }
    fn undo(&mut self, id: StructureEntryId) -> Result<Option<Outcome>, CallError> {
        self.host.undo(id)
    }
    fn redo(&mut self, id: StructureEntryId) -> Result<Option<Outcome>, CallError> {
        self.host.redo(id)
    }
    fn source(&self, id: StructureEntryId) -> Option<&ImportedBlock> {
        self.host.source(id)
    }
    fn has_window(&self) -> bool {
        self.host.has_window()
    }
    fn projects(&self) -> Option<&dyn Projects> {
        self.host.projects()
    }
    fn projects_mut(&mut self) -> Option<&mut dyn Projects> {
        self.host.projects_mut()
    }
    fn agent(&self) -> Option<Agent> {
        self.agent.clone()
    }
    fn audit(&mut self, project: Option<ProjectId>, record: AuditRecord) {
        self.host.audit(project, record);
    }
    fn writable(&self, path: &str) -> Result<PathBuf, CallError> {
        self.host.writable(path)
    }
    fn permissions(&self, client: &Client) -> Permissions {
        self.host.permissions(client)
    }
    fn ask(&mut self, client: &Client, question: &str, answer: Answer) -> bool {
        self.host.ask(client, question, answer)
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
    /// A write that only adds, such as a new site or structure, rather
    /// than changing or removing what is there (MCP's `destructiveHint`).
    pub additive: bool,
    /// Calling it again with the same arguments changes nothing more, as
    /// for setting a value (MCP's `idempotentHint`). Queries always are.
    pub idempotent: bool,
}

impl Spec {
    pub fn new(name: &'static str, description: &'static str, effect: Effect) -> Self {
        Self {
            name,
            description,
            effect,
            availability: Availability::Both,
            additive: false,
            idempotent: matches!(effect, Effect::ReadOnly | Effect::View | Effect::External),
        }
    }

    pub fn additive(mut self) -> Self {
        self.additive = true;
        self
    }

    pub fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }

    pub fn only(mut self, availability: Availability) -> Self {
        self.availability = availability;
        self
    }
}

type Handler = Box<dyn Fn(&mut dyn Host, Value) -> Result<CallResult, CallError> + Send + Sync>;
type Preflight = Box<dyn Fn(&dyn Host, Value) -> Result<String, CallError> + Send + Sync>;

/// One operation that every entry point can call: the interface, scripts,
/// the command line, MCP clients and the built-in assistant (design §4).
pub struct Capability {
    pub name: &'static str,
    pub description: &'static str,
    pub effect: Effect,
    pub availability: Availability,
    /// See [`Spec::additive`].
    pub additive: bool,
    /// See [`Spec::idempotent`].
    pub idempotent: bool,
    pub input_schema: Value,
    pub output_schema: Value,
    handler: Handler,
    /// For capabilities that need the user's confirmation: checks the
    /// arguments and says what the call would do.
    preflight: Option<Preflight>,
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

impl Capability {
    fn available(&self, host: &dyn Host) -> Result<(), CallError> {
        let available = match self.availability {
            Availability::Gui => host.has_window(),
            Availability::Headless => !host.has_window(),
            Availability::Both => true,
        };
        if available {
            Ok(())
        } else {
            Err(CallError::new(
                "unavailable",
                format!("{} is not available in this mode", self.name),
            ))
        }
    }
}

fn arguments<I: DeserializeOwned>(input: Value) -> Result<I, CallError> {
    serde_json::from_value(input)
        .map_err(|error| CallError::new("invalid_arguments", error.to_string()))
}

/// The project and the entry a call is about, for its audit record: the
/// structure it names, or the project it names or opens.
fn audit_target(
    host: &dyn Host,
    input: &Value,
    output: Option<&Value>,
) -> (Option<ProjectId>, Option<EntryId>) {
    let structure = input["structure"]
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
        .map(StructureEntryId);
    if let Some(id) = structure
        && let Some(entry) = host
            .projects()
            .and_then(|p| p.entries().into_iter().find(|e| e.structure == id))
    {
        return (Some(entry.project), Some(entry.entry));
    }
    let project = [Some(input), output]
        .into_iter()
        .flatten()
        .find_map(|value| value["project"].as_str().and_then(ProjectId::parse));
    (project, None)
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
        self.add(spec, handler, None);
    }

    /// Adds a capability that runs only once the user confirmed the call
    /// (design §7.1): `preflight` checks the arguments and says, in one
    /// English sentence for the user, what the call would do.
    pub fn register_confirmed<I, O, P, F>(&mut self, spec: Spec, preflight: P, handler: F)
    where
        I: Schema + DeserializeOwned,
        O: Schema + Serialize,
        P: Fn(&dyn Host, I) -> Result<String, CallError> + Send + Sync + 'static,
        F: Fn(&mut dyn Host, I) -> Result<Output<O>, CallError> + Send + Sync + 'static,
    {
        let preflight: Preflight =
            Box::new(move |host: &dyn Host, input: Value| preflight(host, arguments(input)?));
        self.add(spec, handler, Some(preflight));
    }

    fn add<I, O, F>(&mut self, spec: Spec, handler: F, preflight: Option<Preflight>)
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
        assert!(
            spec.effect != Effect::External || preflight.is_some(),
            "{} has external effects, so it must say what the user confirms",
            spec.name
        );
        let handler = move |host: &mut dyn Host, input: Value| {
            let output = handler(host, arguments(input)?)?;
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
            additive: spec.additive,
            idempotent: spec.idempotent,
            input_schema: I::schema(),
            output_schema: O::schema(),
            handler: Box::new(handler),
            preflight,
        });
    }

    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    pub fn get(&self, name: &str) -> Option<&Capability> {
        self.capabilities.iter().find(|c| c.name == name)
    }

    fn find(&self, name: &str) -> Result<&Capability, CallError> {
        self.get(name).ok_or_else(|| {
            CallError::new("unknown_capability", format!("no capability named {name}"))
        })
    }

    /// Calls a capability as the user at the keyboard, who may run
    /// anything; JSON arguments, `null` counting as `{}`.
    pub fn call(
        &self,
        host: &mut dyn Host,
        name: &str,
        input: Value,
    ) -> Result<CallResult, CallError> {
        self.call_as(host, &Context::user(), name, input)
    }

    /// Calls a capability in `context`: an AI client runs only what the
    /// user allowed it, and irreversible capabilities only once confirmed
    /// (design §7.2), whatever its model says. Every call of a client,
    /// refused ones too, leaves a record for the audit log
    /// ([`Host::audit`]).
    pub fn call_as(
        &self,
        host: &mut dyn Host,
        context: &Context,
        name: &str,
        input: Value,
    ) -> Result<CallResult, CallError> {
        let capability = self.find(name)?;
        let input = if input.is_null() { json!({}) } else { input };
        let (project, entry) = audit_target(host, &input, None);
        let result = capability
            .available(host)
            .and_then(|()| context.permit(name, capability.effect))
            .and_then(|()| {
                let mut calling = Calling {
                    host,
                    agent: context.client_info().map(policy::Client::agent),
                };
                (capability.handler)(&mut calling, input.clone())
            });
        if let Some(client) = context.client_info() {
            let project = project.or_else(|| {
                let output = result.as_ref().ok().map(|r| &r.output);
                audit_target(host, &input, output).0
            });
            let record = policy::record(
                client,
                name,
                entry,
                &input,
                capability.effect,
                context.confirmed,
                result.as_ref(),
            );
            host.audit(project, record);
        }
        result
    }

    /// Records that a client's call of `name` was refused before it could
    /// run, e.g. because its arguments did not pass the confirmation's
    /// checks or the user could not be asked; returns the error.
    pub fn refuse(
        &self,
        host: &mut dyn Host,
        context: &Context,
        name: &str,
        input: &Value,
        error: CallError,
    ) -> CallError {
        if let (Some(capability), Some(client)) = (self.get(name), context.client_info()) {
            let (project, entry) = audit_target(host, input, None);
            let record = policy::record(
                client,
                name,
                entry,
                input,
                capability.effect,
                context.confirmed,
                Err(&error),
            );
            host.audit(project, record);
        }
        error
    }

    /// What the user is asked to confirm before `name` runs with `input`:
    /// one English sentence, or `None` for a capability that needs no
    /// confirmation. Checks the arguments first, so that the user is not
    /// asked about a call that would fail.
    pub fn confirmation(
        &self,
        host: &dyn Host,
        name: &str,
        input: Value,
    ) -> Result<Option<String>, CallError> {
        let capability = self.find(name)?;
        capability.available(host)?;
        let input = if input.is_null() { json!({}) } else { input };
        capability
            .preflight
            .as_ref()
            .map(|preflight| preflight(host, input))
            .transpose()
    }
}
