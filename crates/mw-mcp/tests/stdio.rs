//! The server as MCP clients see it: the `mw-mcp` binary, driven over stdio
//! by rmcp's client, both through `initialize` (protocol 2025-11-25 and
//! earlier, which current clients speak) and through `server/discover`
//! (2026-07-28). It opens the sample projects, installed in a temporary
//! directory, and the CIF files in `fixtures/`; edits them, and writes
//! files once the client's user confirms through elicitation.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mw_automation::{Availability, Effect, Registry};
use mw_llm::ToolCall;
use mw_llm::schema::{check_schema, validate_arguments};
use mw_mcp::{UNTRUSTED, tool_name};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, ElicitRequestParams,
    ElicitResult, ElicitationAction, ElicitationCapability, Implementation, ProtocolVersion,
    ReadResourceRequestParams, ResourceContents, Tool,
};
use rmcp::service::{RequestContext, RunningService};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt, ErrorData as McpError, RoleClient,
    ServiceExt,
};
use serde_json::{Value, json};
use tokio::process::Command;

type Client = RunningService<RoleClient, ()>;

/// A client whose user answers every confirmation the same way, and which
/// keeps the questions.
#[derive(Clone)]
struct Answering {
    accept: bool,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Answering {
    fn new(accept: bool) -> Self {
        Self {
            accept,
            asked: Arc::default(),
        }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl ClientHandler for Answering {
    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, McpError> {
        if let ElicitRequestParams::FormElicitationParams { message, .. } = &request {
            self.asked.lock().unwrap().push(message.clone());
        }
        let action = if self.accept {
            ElicitationAction::Accept
        } else {
            ElicitationAction::Decline
        };
        Ok(ElicitResult::new(action))
    }

    fn get_info(&self) -> ClientConfig {
        let mut capabilities = ClientCapabilities::default();
        capabilities.elicitation = Some(ElicitationCapability::default());
        let client = Implementation::new("test-client", "1.0").with_title("Test Client");
        ClientConfig::new(capabilities, client)
    }
}

/// The ids in the sample project `tio2.mwproj`.
const TIO2: &str = "6d2a51c0-93e4-4b7a-8f10-2c4e5a6b7c00";
const RUTILE: &str = "6d2a51c0-93e4-4b7a-8f10-2c4e5a6b7c01";
const SUPERCELL: &str = "6d2a51c0-93e4-4b7a-8f10-2c4e5a6b7c02";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture(name: &str) -> String {
    fixtures().join(name).to_string_lossy().into_owned()
}

/// The sample projects in a directory of their own.
struct Projects {
    dir: tempfile::TempDir,
    tio2: PathBuf,
}

impl Projects {
    fn install() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let installed = mw_project_file::samples::install(dir.path()).unwrap();
        let tio2 = installed[0].canonicalize().unwrap();
        Self { dir, tio2 }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

fn transport(projects: &Projects) -> TokioChildProcess {
    let command = Command::new(env!("CARGO_BIN_EXE_mw-mcp")).configure(|command| {
        command
            .arg("--allow")
            .arg(projects.path())
            .arg("--allow")
            .arg(fixtures());
    });
    TokioChildProcess::new(command).expect("the server starts")
}

/// A client that begins with `initialize`.
async fn start(projects: &Projects) -> Client {
    ().serve(transport(projects)).await.expect("initialize")
}

/// A client that begins with `server/discover` and sends its version and
/// capabilities with every request.
async fn start_stateless(projects: &Projects) -> Client {
    let lifecycle = ClientLifecycleMode::Discover {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
    };
    ().serve_with_lifecycle(transport(projects), lifecycle)
        .await
        .expect("discover")
}

async fn call<H: ClientHandler>(
    client: &RunningService<RoleClient, H>,
    tool: &str,
    arguments: Value,
) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object");
    };
    let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
    client.call_tool(params).await.expect("a tool result")
}

fn conforms(schema: &Value, value: &Value, what: &str) {
    let call = ToolCall::new("check", "check", value.to_string());
    if let Err(error) = validate_arguments(schema, &call) {
        panic!("{what} does not match its schema: {error}\n{value:#}");
    }
}

/// A successful result's structured output, checked against the tool's
/// output schema and against the JSON text that comes with it.
fn output<'a>(tools: &[Tool], tool: &str, result: &'a CallToolResult) -> &'a Value {
    assert_ne!(result.is_error, Some(true), "{tool}: {result:#?}");
    let output = result.structured_content.as_ref().expect("structured");
    let schema = tools
        .iter()
        .find(|t| t.name == tool)
        .and_then(|t| t.output_schema.as_ref())
        .expect("an output schema");
    conforms(&Value::Object((**schema).clone()), output, tool);
    let json: Value = serde_json::from_str(&text(result, 1)).unwrap();
    assert_eq!(&json, output);
    assert!(untrusted(result.meta.as_ref().map(|m| &m.0)), "{tool}");
    output
}

/// The text of a result's content block.
fn text(result: &CallToolResult, block: usize) -> String {
    result.content[block].as_text().expect("text").text.clone()
}

/// An error result's code.
fn error_code(result: &CallToolResult) -> String {
    assert_eq!(result.is_error, Some(true), "{result:#?}");
    let error: Value = serde_json::from_str(&text(result, 0)).unwrap();
    error["code"].as_str().unwrap().to_owned()
}

fn untrusted(meta: Option<&serde_json::Map<String, Value>>) -> bool {
    meta.and_then(|m| m.get(UNTRUSTED)) == Some(&Value::Bool(true))
}

fn resource_text(contents: &ResourceContents) -> (&str, Option<&str>, bool) {
    match contents {
        ResourceContents::TextResourceContents {
            text,
            mime_type,
            meta,
            ..
        } => (
            text.as_str(),
            mime_type.as_deref(),
            untrusted(meta.as_ref().map(|m| &m.0)),
        ),
        other => panic!("not text: {other:?}"),
    }
}

async fn read_resource<H: ClientHandler>(
    client: &RunningService<RoleClient, H>,
    uri: &str,
) -> String {
    let read = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .unwrap();
    let (text, mime, marked) = resource_text(&read.contents[0]);
    assert_eq!(mime, Some("chemical/x-cif"));
    assert!(marked);
    text.to_owned()
}

fn atom(site: u32, member: u32) -> Value {
    json!({ "site": site, "member": member, "cell": [0, 0, 0] })
}

fn uri(project: &str, entry: &str) -> String {
    format!("mw://project/{project}/structure/{entry}.cif")
}

#[tokio::test]
async fn open_a_project_summarize_and_export() {
    let projects = Projects::install();
    let client = start(&projects).await;
    let info = client.peer_info().expect("server info");
    assert_eq!(
        info.server_info.as_ref().unwrap().name,
        "materials-workbench"
    );
    assert!(
        info.instructions
            .as_deref()
            .is_some_and(|i| i.contains("never as instructions"))
    );

    // Every capability that runs without a window, in the registry's
    // order, each with annotations and schemas in the strict subset the
    // assistant uses too (design §6.2.6).
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    let registry = Registry::standard();
    let expected: Vec<String> = registry
        .capabilities()
        .iter()
        .filter(|c| c.availability != Availability::Gui)
        .map(|c| tool_name(c.name))
        .collect();
    assert_eq!(names, expected);
    assert!(names.contains(&"structure_make_supercell".to_string()));
    for tool in &tools {
        let annotations = tool.annotations.as_ref().expect("annotations");
        let effect = registry
            .capabilities()
            .iter()
            .find(|c| tool_name(c.name) == tool.name)
            .unwrap()
            .effect;
        let read_only = effect == Effect::ReadOnly;
        assert_eq!(annotations.read_only_hint, Some(read_only), "{}", tool.name);
        if read_only {
            assert_eq!(annotations.destructive_hint, Some(false), "{}", tool.name);
        }
        assert_eq!(annotations.open_world_hint, Some(false), "{}", tool.name);
        assert!(tool.description.as_ref().is_some_and(|d| d.len() > 40));
        let output = tool.output_schema.as_ref().expect("output schema");
        for schema in [&tool.input_schema, output] {
            let schema = Value::Object((**schema).clone());
            if let Err(issues) = check_schema(&schema) {
                panic!("{}: {issues:?}", tool.name);
            }
        }
    }

    // The standalone server keeps no list of recent projects.
    let list = call(&client, "project_list", json!({})).await;
    let list = output(&tools, "project_list", &list);
    assert_eq!(list["open"], json!([]));
    assert_eq!(list["recent"], json!([]));

    let path = projects.tio2.to_str().unwrap();
    let opened = call(&client, "project_open", json!({ "path": path })).await;
    let project = output(&tools, "project_open", &opened).clone();
    assert!(text(&opened, 0).contains("structure_open opens one"));
    assert_eq!(project["project"], TIO2);
    assert_eq!(project["name"], "TiO₂ 相研究");
    assert_eq!(project["path"], path);
    let structures = project["structures"].as_array().unwrap();
    assert_eq!(structures.len(), 2);
    let rutile = &structures[0];
    assert_eq!(rutile["name"], "金红石");
    assert_eq!(rutile["open"], false);
    assert_eq!(rutile["resource"], uri(TIO2, RUTILE));
    assert_eq!(structures[1]["origin"], "2×2×2 supercell of 金红石");
    let id = rutile["structure"].as_u64().unwrap();
    // Opening it again gives the same ids.
    let again = call(&client, "project_open", json!({ "path": path })).await;
    assert_eq!(output(&tools, "project_open", &again), &project);

    // A structure is opened before the other tools use it.
    let summary = call(&client, "structure_summary", json!({ "structure": id })).await;
    assert_eq!(error_code(&summary), "structure_not_open");
    assert!(text(&summary, 0).contains("structure_open opens it"));
    let open = call(&client, "structure_open", json!({ "structure": id })).await;
    let open = output(&tools, "structure_open", &open);
    assert_eq!(open["project"], "TiO₂ 相研究");
    assert_eq!(open["note"], Value::Null);

    let list = call(&client, "structure_list", json!({})).await;
    let list = output(&tools, "structure_list", &list);
    let open: Vec<&Value> = list["structures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| &s["open"])
        .collect();
    assert_eq!(open, [true, false]);
    let list = call(&client, "project_list", json!({})).await;
    let list = output(&tools, "project_list", &list);
    assert_eq!(list["open"][0]["project"], TIO2);
    assert_eq!(list["open"][0]["structures"], 2);

    let summary = call(&client, "structure_summary", json!({ "structure": id })).await;
    assert!(text(&summary, 0).contains("P 4_2/m n m (No. 136)"));
    let summary = output(&tools, "structure_summary", &summary);
    assert_eq!(summary["name"], "金红石");
    assert_eq!(summary["formula"], "O2Ti");
    assert_eq!(summary["formula_units"], 2);
    assert_eq!(summary["space_group"]["number"], 136);
    assert_eq!(summary["atoms_in_cell"], 6);

    // The equatorial Ti–O bond: √2 · 0.3048 · 4.594 Å.
    let atoms = json!({ "structure": id, "atoms": [atom(1, 0), atom(2, 0)] });
    let distance = call(&client, "structure_measure_distance", atoms).await;
    let distance = output(&tools, "structure_measure_distance", &distance)["distance"]
        .as_f64()
        .unwrap();
    let expected = 2f64.sqrt() * 0.3048 * 4.594;
    assert!((distance - expected).abs() < 1e-9, "{distance}");

    // Read again from the project's copy of rutile.cif.
    let diagnostics = json!({ "structure": id, "offset": 0, "limit": 50 });
    let diagnostics = call(&client, "structure_diagnostics", diagnostics).await;
    let diagnostics = output(&tools, "structure_diagnostics", &diagnostics);
    assert_eq!(diagnostics["block"], "rutile");
    assert_eq!(diagnostics["errors"], 0);

    let export = json!({ "structure": id, "format": "poscar" });
    let export = call(&client, "structure_export", export).await;
    let export = output(&tools, "structure_export", &export);
    assert_eq!(export["extension"], "vasp");
    let poscar: Vec<&str> = export["text"].as_str().unwrap().lines().collect();
    assert_eq!(poscar[0], "金红石 · TiO₂ 相研究");
    assert_eq!(
        poscar[5].split_whitespace().collect::<Vec<_>>(),
        ["Ti", "O"]
    );
    assert_eq!(poscar[6].split_whitespace().collect::<Vec<_>>(), ["2", "4"]);
    assert_eq!(poscar.len(), 8 + 6);

    // The open structures' CIF text as resources, by the projects' ids.
    let resources = client.list_all_resources().await.unwrap();
    let uris: Vec<&str> = resources.iter().map(|r| r.uri.as_str()).collect();
    assert_eq!(uris, [uri(TIO2, RUTILE)]);
    assert_eq!(resources[0].title.as_deref(), Some("金红石"));
    let templates = client.list_all_resource_templates().await.unwrap();
    assert_eq!(
        templates[0].uri_template,
        "mw://project/{project}/structure/{structure}.cif"
    );
    let cif = read_resource(&client, &uri(TIO2, RUTILE)).await;
    assert!(cif.contains("data_rutile"), "{cif}");
    // Unchanged since the import: the source file's other fields are back.
    assert!(cif.contains("_chemical_name_mineral"), "{cif}");
    // Reading a structure that is not open opens it.
    let supercell = read_resource(&client, &uri(TIO2, SUPERCELL)).await;
    assert!(supercell.contains("Ti1_1"), "{supercell}");
    assert_eq!(client.list_all_resources().await.unwrap().len(), 2);
    // The supercell is in P1; its atoms have rutile's symmetry, which the
    // preview finds without changing anything.
    let supercell = structures[1]["structure"].as_u64().unwrap();
    let detect = json!({ "structure": supercell, "tolerance": 0.0, "standardize": true });
    let detected = call(&client, "structure_detect_symmetry", detect).await;
    let detected = output(&tools, "structure_detect_symmetry", &detected);
    assert_eq!(detected["detected"]["number"], 136);
    assert_eq!(detected["atoms_in_cell"], 6);
    assert_eq!(detected["cell_changed"], true);
    let summary = call(
        &client,
        "structure_summary",
        json!({ "structure": supercell }),
    )
    .await;
    assert_eq!(
        output(&tools, "structure_summary", &summary)["operations"],
        1
    );
    let missing = ReadResourceRequestParams::new(uri(TIO2, TIO2));
    assert!(client.read_resource(missing).await.is_err());

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stateless_clients_get_the_same_answers() {
    let projects = Projects::install();
    let client = start_stateless(&projects).await;
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 33);
    let path = projects.tio2.to_str().unwrap();
    let opened = call(&client, "project_open", json!({ "path": path })).await;
    let id = output(&tools, "project_open", &opened)["structures"][0]["structure"].clone();
    let open = call(&client, "structure_open", json!({ "structure": id })).await;
    output(&tools, "structure_open", &open);
    let summary = call(&client, "structure_summary", json!({ "structure": id })).await;
    assert_eq!(
        output(&tools, "structure_summary", &summary)["formula"],
        "O2Ti"
    );
    let cif = read_resource(&client, &uri(TIO2, RUTILE)).await;
    assert!(cif.contains("data_rutile"));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn files_outside_projects() {
    let projects = Projects::install();
    let client = start(&projects).await;
    let tools = client.list_all_tools().await.unwrap();
    let opened = call(
        &client,
        "file_open",
        json!({ "path": fixture("rutile.cif") }),
    )
    .await;
    let structure = &output(&tools, "file_open", &opened)["structures"][0];
    assert_eq!(structure["name"], "rutile");
    assert_eq!(structure["block"], "rutile");
    let id = structure["structure"].as_u64().unwrap();
    let resource = structure["resource"].as_str().unwrap().to_owned();
    assert!(resource.starts_with("mw://project/"), "{resource}");
    // Opening an unchanged file again gives the same structure.
    let again = call(
        &client,
        "file_open",
        json!({ "path": fixture("rutile.cif") }),
    )
    .await;
    assert_eq!(
        output(&tools, "file_open", &again),
        output(&tools, "file_open", &opened)
    );
    assert!(text(&again, 0).starts_with("Already open."));

    // In an untitled project that is never saved.
    let list = call(&client, "project_list", json!({})).await;
    let list = output(&tools, "project_list", &list);
    assert_eq!(list["open"][0]["name"], "Opened files");
    assert_eq!(list["open"][0]["path"], Value::Null);
    let summary = call(&client, "structure_summary", json!({ "structure": id })).await;
    assert_eq!(
        output(&tools, "structure_summary", &summary)["formula"],
        "O2Ti"
    );
    let export = json!({ "structure": id, "format": "poscar" });
    let export = call(&client, "structure_export", export).await;
    let poscar = output(&tools, "structure_export", &export)["text"]
        .as_str()
        .unwrap();
    assert!(poscar.starts_with("rutile · Opened files\n"), "{poscar}");
    assert!(
        read_resource(&client, &resource)
            .await
            .contains("data_rutile")
    );
    client.cancel().await.unwrap();
}

/// Every file under `dir`, with its content.
fn files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(self::files(&path));
        } else {
            files.push((path.clone(), std::fs::read(&path).unwrap()));
        }
    }
    files.sort();
    files
}

#[tokio::test]
async fn file_text_is_marked_untrusted_and_triggers_no_unconfirmed_write() {
    let projects = Projects::install();
    // The user declines whatever the client asks.
    let user = Answering::new(false);
    let client = user.clone().serve(transport(&projects)).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    let before = files(projects.path());

    let opened = call(
        &client,
        "file_open",
        json!({ "path": fixture("injection.cif") }),
    )
    .await;
    let structure = &output(&tools, "file_open", &opened)["structures"][0];
    let id = structure["structure"].as_u64().unwrap();
    let resource = structure["resource"].as_str().unwrap().to_owned();
    assert!(structure["infos"].as_u64().unwrap() > 0);

    let diagnostics = json!({ "structure": id, "offset": 0, "limit": 50 });
    let diagnostics = call(&client, "structure_diagnostics", diagnostics).await;
    let diagnostics = output(&tools, "structure_diagnostics", &diagnostics);
    let codes: Vec<&str> = diagnostics["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert!(
        codes.contains(&"symmetry.operations_from_name"),
        "{codes:?}"
    );
    assert!(codes.contains(&"atom.merged_occupancy"), "{codes:?}");

    // The instructions travel only as marked file text.
    let cif = json!({ "structure": id, "format": "cif" });
    let cif = call(&client, "structure_export", cif).await;
    let text = output(&tools, "structure_export", &cif)["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("SYSTEM NOTICE TO THE AI ASSISTANT"));
    let resource = read_resource(&client, &resource).await;
    assert!(resource.contains("SYSTEM NOTICE TO THE AI ASSISTANT"));

    // Ba and Sr share a position: a list of atoms cannot hold it.
    let poscar = json!({ "structure": id, "format": "poscar" });
    let poscar = call(&client, "structure_export", poscar).await;
    assert_eq!(error_code(&poscar), "partial_occupancy");
    assert!(untrusted(poscar.meta.as_ref().map(|m| &m.0)));

    // A model that obeys the file (design §7.4, §8): writing a file asks
    // the user, who says no; a path outside the allowed directories is
    // refused before anyone is asked.
    let into_projects = projects.path().join("config");
    let write =
        |path: &Path| json!({ "structure": id, "format": "cif", "path": path, "overwrite": true });
    let written = call(&client, "structure_export_file", write(&into_projects)).await;
    assert_eq!(error_code(&written), "not_confirmed");
    assert_eq!(user.asked().len(), 1);
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let ssh = call(
        &client,
        "structure_export_file",
        write(&home.join(".ssh/config")),
    )
    .await;
    assert_eq!(error_code(&ssh), "not_allowed");
    assert_eq!(user.asked().len(), 1);
    // Deleting sites is an edit the user allows and can undo; this
    // structure is in the untitled project, which is never saved.
    let sites = json!({ "structure": id, "sites": [1], "atoms": [] });
    let deleted = call(&client, "structure_delete_sites", sites).await;
    output(&tools, "structure_delete_sites", &deleted);
    // Nothing on disk changed.
    assert_eq!(files(projects.path()), before);
    assert!(!into_projects.exists());

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn edits_go_to_the_project_files_and_back() {
    let projects = Projects::install();
    let client = start(&projects).await;
    let tools = client.list_all_tools().await.unwrap();
    let path = projects.tio2.to_str().unwrap();
    call(&client, "project_open", json!({ "path": path })).await;
    call(&client, "structure_open", json!({ "structure": 1 })).await;
    let set = |a: f64| json!({ "structure": 1, "values": [{ "parameter": "a", "value": a }] });
    let edited = call(&client, "structure_set_cell", set(4.7)).await;
    assert!(
        text(&edited, 0).starts_with("The cell is now a = 4.7"),
        "{}",
        text(&edited, 0)
    );
    output(&tools, "structure_set_cell", &edited);
    let saved = |projects: &Projects| {
        let file = projects.tio2.join(format!("structures/{RUTILE}.json"));
        std::fs::read_to_string(file).unwrap()
    };
    assert!(saved(&projects).contains("4.7"));
    let undone = call(&client, "history_undo", json!({ "structure": 1 })).await;
    assert_eq!(
        output(&tools, "history_undo", &undone)["command"],
        "set_cell"
    );
    assert!(saved(&projects).contains("4.594"));

    // A supercell joins the project, and its resource is listed.
    let make = json!({ "structure": 1, "matrix": [[1, 0, 0], [0, 1, 0], [0, 0, 3]], "name": "" });
    let made = call(&client, "structure_make_supercell", make).await;
    let made = output(&tools, "structure_make_supercell", &made);
    assert_eq!(made["sites"], 18);
    let resources = client.list_all_resources().await.unwrap();
    assert_eq!(resources.len(), 2);
    let manifest = std::fs::read_to_string(projects.tio2.join("project.json")).unwrap();
    assert!(manifest.contains("金红石 1×1×3 supercell"), "{manifest}");
    // The project's audit log names the client.
    let log = std::fs::read_to_string(projects.tio2.join("audit.jsonl")).unwrap();
    assert!(log.contains("structure.make_supercell"), "{log}");
    assert!(log.contains("rmcp"), "{log}");
    client.cancel().await.unwrap();
}

/// Writes the rutile's POSCAR into the projects' directory through a client
/// whose user answers `accept`; returns the questions and the result.
async fn export_poscar(stateless: bool, accept: bool) -> (Vec<String>, CallToolResult, bool) {
    let projects = Projects::install();
    let user = Answering::new(accept);
    let client = if stateless {
        let lifecycle = ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        };
        user.clone()
            .serve_with_lifecycle(transport(&projects), lifecycle)
            .await
            .unwrap()
    } else {
        user.clone().serve(transport(&projects)).await.unwrap()
    };
    let path = projects.tio2.to_str().unwrap();
    call(&client, "project_open", json!({ "path": path })).await;
    call(&client, "structure_open", json!({ "structure": 1 })).await;
    let target = projects.path().join("POSCAR");
    let export = json!({ "structure": 1, "format": "poscar", "path": target, "overwrite": false });
    let result = call(&client, "structure_export_file", export).await;
    let written = target.exists();
    client.cancel().await.unwrap();
    (user.asked(), result, written)
}

#[tokio::test]
async fn writing_a_file_asks_the_user_every_time() {
    for stateless in [false, true] {
        let (asked, result, written) = export_poscar(stateless, true).await;
        assert_eq!(asked.len(), 1, "stateless {stateless}");
        assert!(
            asked[0].starts_with("Test Client asks: Write 金红石 as POSCAR"),
            "{asked:?}"
        );
        assert!(asked[0].contains("POSCAR ("), "{asked:?}");
        assert_ne!(result.is_error, Some(true), "{result:#?}");
        assert!(written, "stateless {stateless}");

        let (asked, result, written) = export_poscar(stateless, false).await;
        assert_eq!(asked.len(), 1);
        assert_eq!(error_code(&result), "not_confirmed");
        assert!(!written, "stateless {stateless}");
    }
}

#[tokio::test]
async fn clients_that_cannot_ask_cannot_write_files() {
    let projects = Projects::install();
    // rmcp's plain client declares no elicitation.
    let client = start(&projects).await;
    let path = projects.tio2.to_str().unwrap();
    call(&client, "project_open", json!({ "path": path })).await;
    call(&client, "structure_open", json!({ "structure": 1 })).await;
    let target = projects.path().join("out.cif");
    let export = json!({ "structure": 1, "format": "cif", "path": target, "overwrite": false });
    let refused = call(&client, "structure_export_file", export).await;
    assert_eq!(error_code(&refused), "confirmation_unavailable");
    assert!(!target.exists());
    // The refusal is in the audit log, once the session writes the project.
    let edit = json!({ "structure": 1, "values": [{ "parameter": "c", "value": 3.0 }] });
    call(&client, "structure_set_cell", edit).await;
    let log = std::fs::read_to_string(projects.tio2.join("audit.jsonl")).unwrap();
    assert!(log.contains("confirmation_unavailable"), "{log}");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn refusals_are_tool_errors_the_model_can_read() {
    let projects = Projects::install();
    let client = start(&projects).await;
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let outside = call(&client, "file_open", json!({ "path": manifest })).await;
    assert_eq!(error_code(&outside), "not_allowed");
    let missing = call(&client, "file_open", json!({ "path": fixture("none.cif") })).await;
    assert_eq!(error_code(&missing), "not_found");
    let readme = call(
        &client,
        "file_open",
        json!({ "path": fixture("README.md") }),
    )
    .await;
    assert_eq!(error_code(&readme), "unsupported_format");
    let extra = json!({ "path": fixture("rutile.cif"), "mode": "write" });
    assert_eq!(
        error_code(&call(&client, "file_open", extra).await),
        "invalid_arguments"
    );

    let crate_dir = env!("CARGO_MANIFEST_DIR");
    let outside = call(&client, "project_open", json!({ "path": crate_dir })).await;
    assert_eq!(error_code(&outside), "not_allowed");
    let not_a_project = call(&client, "project_open", json!({ "path": fixtures() })).await;
    assert_eq!(error_code(&not_a_project), "not_a_project");
    let missing = projects.path().join("none.mwproj");
    let missing = call(&client, "project_open", json!({ "path": missing })).await;
    assert_eq!(error_code(&missing), "not_found");

    let unknown = call(&client, "structure_summary", json!({ "structure": 7 })).await;
    assert_eq!(error_code(&unknown), "unknown_structure");
    let unknown = call(&client, "structure_open", json!({ "structure": 7 })).await;
    assert_eq!(error_code(&unknown), "unknown_structure");
    let wrong = call(&client, "structure_summary", json!({ "id": 1 })).await;
    assert_eq!(error_code(&wrong), "invalid_arguments");
    // Views need the application's window, so a call to one is a protocol
    // error here.
    let edit = CallToolRequestParams::new("no_such_tool");
    assert!(client.call_tool(edit).await.is_err());
    client.cancel().await.unwrap();
}
