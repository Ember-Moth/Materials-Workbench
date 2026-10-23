//! The server as MCP clients see it: the `mw-mcp` binary, driven over stdio
//! by rmcp's client, both through `initialize` (protocol 2025-11-25 and
//! earlier, which current clients speak) and through `server/discover`
//! (2026-07-28).

use std::path::PathBuf;

use mw_llm::ToolCall;
use mw_llm::schema::{check_schema, validate_arguments};
use mw_mcp::UNTRUSTED;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ProtocolVersion, ReadResourceRequestParams,
    ResourceContents, Tool,
};
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{ClientLifecycleMode, ClientServiceExt, RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::process::Command;

type Client = RunningService<RoleClient, ()>;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture(name: &str) -> String {
    fixtures().join(name).to_string_lossy().into_owned()
}

fn transport() -> TokioChildProcess {
    let command = Command::new(env!("CARGO_BIN_EXE_mw-mcp")).configure(|command| {
        command.arg("--allow").arg(fixtures());
    });
    TokioChildProcess::new(command).expect("the server starts")
}

/// A client that begins with `initialize`.
async fn start() -> Client {
    ().serve(transport()).await.expect("initialize")
}

/// A client that begins with `server/discover` and sends its version and
/// capabilities with every request.
async fn start_stateless() -> Client {
    let lifecycle = ClientLifecycleMode::Discover {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
    };
    ().serve_with_lifecycle(transport(), lifecycle)
        .await
        .expect("discover")
}

async fn call(client: &Client, tool: &str, arguments: Value) -> CallToolResult {
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

fn atom(site: u32, member: u32) -> Value {
    json!({ "site": site, "member": member, "cell": [0, 0, 0] })
}

#[tokio::test]
async fn open_summarize_measure_and_export() {
    let client = start().await;
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

    // Only read-only tools, each with annotations and schemas in the
    // strict subset the assistant uses too (design §6.2.6).
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "file_open",
            "structure_list",
            "structure_summary",
            "structure_sites",
            "structure_site_symmetry",
            "structure_measure_distance",
            "structure_measure_angle",
            "structure_neighbors",
            "structure_diagnostics",
            "structure_export",
        ]
    );
    for tool in &tools {
        let annotations = tool.annotations.as_ref().expect("annotations");
        assert_eq!(annotations.read_only_hint, Some(true), "{}", tool.name);
        assert_eq!(annotations.destructive_hint, Some(false), "{}", tool.name);
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
    let uri = format!("mw://project/session/structure/{id}.cif");
    assert_eq!(structure["resource"], uri.as_str());
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

    let list = call(&client, "structure_list", json!({})).await;
    let list = output(&tools, "structure_list", &list);
    assert_eq!(list["structures"].as_array().unwrap().len(), 1);

    let summary = call(&client, "structure_summary", json!({ "structure": id })).await;
    assert!(text(&summary, 0).contains("P 4_2/m n m (No. 136)"));
    let summary = output(&tools, "structure_summary", &summary);
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
    assert_eq!(poscar[0], "rutile · Opened files");
    assert_eq!(
        poscar[5].split_whitespace().collect::<Vec<_>>(),
        ["Ti", "O"]
    );
    assert_eq!(poscar[6].split_whitespace().collect::<Vec<_>>(), ["2", "4"]);
    assert_eq!(poscar.len(), 8 + 6);

    // The CIF text as a resource.
    let resources = client.list_all_resources().await.unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].uri, uri);
    let templates = client.list_all_resource_templates().await.unwrap();
    assert_eq!(
        templates[0].uri_template,
        "mw://project/{project}/structure/{structure}.cif"
    );
    let read = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .unwrap();
    let (cif, mime, marked) = resource_text(&read.contents[0]);
    assert!(cif.contains("data_rutile"), "{cif}");
    assert_eq!(mime, Some("chemical/x-cif"));
    assert!(marked);
    let missing = ReadResourceRequestParams::new("mw://project/session/structure/99.cif");
    assert!(client.read_resource(missing).await.is_err());

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn stateless_clients_get_the_same_answers() {
    let client = start_stateless().await;
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 10);
    let opened = call(
        &client,
        "file_open",
        json!({ "path": fixture("rutile.cif") }),
    )
    .await;
    let id = output(&tools, "file_open", &opened)["structures"][0]["structure"].clone();
    let summary = call(&client, "structure_summary", json!({ "structure": id })).await;
    assert_eq!(
        output(&tools, "structure_summary", &summary)["formula"],
        "O2Ti"
    );
    let uri = format!("mw://project/session/structure/{id}.cif");
    let read = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .unwrap();
    assert!(resource_text(&read.contents[0]).0.contains("data_rutile"));
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn file_text_is_marked_untrusted_and_cannot_write() {
    let client = start().await;
    let tools = client.list_all_tools().await.unwrap();
    // A read-only server: whatever a file says, a model has no tool that
    // changes data or writes files (design §7.4, §8).
    assert!(
        tools
            .iter()
            .all(|t| t.annotations.as_ref().unwrap().read_only_hint == Some(true))
    );

    let opened = call(
        &client,
        "file_open",
        json!({ "path": fixture("injection.cif") }),
    )
    .await;
    let structure = &output(&tools, "file_open", &opened)["structures"][0];
    let id = structure["structure"].as_u64().unwrap();
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
    let uri = format!("mw://project/session/structure/{id}.cif");
    let read = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .unwrap();
    let (resource, _, marked) = resource_text(&read.contents[0]);
    assert!(resource.contains("SYSTEM NOTICE TO THE AI ASSISTANT"));
    assert!(marked);

    // Ba and Sr share a position: a list of atoms cannot hold it.
    let poscar = json!({ "structure": id, "format": "poscar" });
    let poscar = call(&client, "structure_export", poscar).await;
    assert_eq!(error_code(&poscar), "partial_occupancy");
    assert!(untrusted(poscar.meta.as_ref().map(|m| &m.0)));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn refusals_are_tool_errors_the_model_can_read() {
    let client = start().await;
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
    let unknown = call(&client, "structure_summary", json!({ "structure": 7 })).await;
    assert_eq!(error_code(&unknown), "unknown_structure");
    let wrong = call(&client, "structure_summary", json!({ "id": 1 })).await;
    assert_eq!(error_code(&wrong), "invalid_arguments");
    // Edits are not tools here, so a call to one is a protocol error.
    let edit = CallToolRequestParams::new("structure_set_cell");
    assert!(client.call_tool(edit).await.is_err());
    client.cancel().await.unwrap();
}
