//! `materials-workbench mcp` as MCP clients start it (ai-integration design
//! §5.1): connected to the application when it listens, here a listener
//! this test starts; otherwise without a window, the protocol on stdio, and
//! the projects the application opened recently, read from its
//! configuration directory (`MW_CONFIG_DIR`). The protocol itself is tested
//! in `mw-mcp`, the application's host in `src/ui_tests.rs`.

use std::fs;
use std::path::Path;
use std::process::Command as StdCommand;

use rmcp::model::{CallToolRequestParams, CallToolResult, ReadResourceRequestParams};
use rmcp::service::RunningService;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::process::Command;

const APP: &str = env!("CARGO_BIN_EXE_materials-workbench");
/// The ids in the sample project `tio2.mwproj`.
const TIO2: &str = "6d2a51c0-93e4-4b7a-8f10-2c4e5a6b7c00";
const RUTILE: &str = "6d2a51c0-93e4-4b7a-8f10-2c4e5a6b7c01";

type Client = RunningService<RoleClient, ()>;

async fn call(client: &Client, tool: &str, arguments: Value) -> Value {
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object");
    };
    let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
    let result: CallToolResult = client.call_tool(params).await.expect("a tool result");
    assert_ne!(result.is_error, Some(true), "{tool}: {result:#?}");
    result.structured_content.expect("structured")
}

/// The configuration directory as the application leaves it, its session
/// listing two recent projects: one where the server may look, one where
/// it may not.
fn configuration(dir: &Path, tio2: &Path) {
    fs::create_dir_all(dir).unwrap();
    let session = json!({
        "version": 1,
        "recent": [
            {
                "path": tio2,
                "id": TIO2,
                "name": "TiO₂ 相研究",
                "opened_at": "2026-10-23T08:00:00Z",
            },
            {
                "path": "/elsewhere/private.mwproj",
                "id": "6d2a51c0-93e4-4b7a-8f10-2c4e5a6b7e00",
                "name": "private",
                "opened_at": "2026-10-22T08:00:00Z",
            },
        ],
        "open": [],
        "active": null,
    });
    fs::write(dir.join("state.json"), session.to_string()).unwrap();
}

#[tokio::test]
async fn recent_project_to_exported_text() {
    let temp = tempfile::tempdir().unwrap();
    let projects = temp.path().join("projects");
    fs::create_dir(&projects).unwrap();
    let installed = mw_project_file::samples::install(&projects).unwrap();
    let tio2 = installed[0].canonicalize().unwrap();
    let config = temp.path().join("config");
    configuration(&config, &tio2);
    let session = fs::read(config.join("state.json")).unwrap();

    let command = Command::new(APP).configure(|command| {
        command
            .arg("mcp")
            .arg("--allow")
            .arg(&projects)
            .env("MW_CONFIG_DIR", &config);
    });
    let transport = TokioChildProcess::new(command).expect("the server starts");
    let client = ().serve(transport).await.expect("initialize");
    let info = client.peer_info().expect("server info");
    assert_eq!(
        info.server_info.as_ref().unwrap().name,
        "materials-workbench"
    );

    // The recent projects the server may open, and how many others there are.
    let list = call(&client, "project_list", json!({})).await;
    assert_eq!(list["recent"].as_array().unwrap().len(), 1);
    let recent = &list["recent"][0];
    assert_eq!(recent["project"], TIO2);
    assert_eq!(recent["open"], false);
    assert_eq!(list["recent_elsewhere"], 1);

    let project = call(&client, "project_open", json!({ "path": recent["path"] })).await;
    assert_eq!(project["name"], "TiO₂ 相研究");
    let id = project["structures"][0]["structure"].clone();
    let uri = format!("mw://project/{TIO2}/structure/{RUTILE}.cif");
    assert_eq!(project["structures"][0]["resource"], uri.as_str());
    call(&client, "structure_open", json!({ "structure": id })).await;
    let summary = call(&client, "structure_summary", json!({ "structure": id })).await;
    assert_eq!(summary["formula"], "O2Ti");
    let poscar = json!({ "structure": id, "format": "poscar" });
    let poscar = call(&client, "structure_export", poscar).await;
    assert!(
        poscar["text"]
            .as_str()
            .unwrap()
            .starts_with("金红石 · TiO₂ 相研究\n"),
        "{poscar:#}"
    );
    let read = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .unwrap();
    let cif = serde_json::to_value(&read.contents[0]).unwrap();
    assert!(cif["text"].as_str().unwrap().contains("data_rutile"));
    client.cancel().await.unwrap();

    // Read-only: the application's session is as it was.
    assert_eq!(fs::read(config.join("state.json")).unwrap(), session);
}

/// Milestone M18: without a window, what clients may change comes from the
/// application's settings, read on every call; changes go to the project's
/// files at once, and the configuration directory stays as it was.
#[tokio::test]
async fn settings_decide_what_clients_change() {
    let temp = tempfile::tempdir().unwrap();
    let projects = temp.path().join("projects");
    fs::create_dir(&projects).unwrap();
    let installed = mw_project_file::samples::install(&projects).unwrap();
    let tio2 = installed[0].canonicalize().unwrap();
    let config = temp.path().join("config");
    configuration(&config, &tio2);
    // rmcp's client calls itself rmcp; the user has not let it change data.
    let settings = |modify: bool| {
        let settings = json!({
            "version": 1,
            "dark": false,
            "language": "zh-CN",
            "mcp": false,
            "ai": {
                "modify_data": true,
                "write_files": true,
                "clients": [{ "name": "rmcp", "modify_data": modify }],
            },
        });
        fs::write(config.join("settings.json"), settings.to_string()).unwrap();
    };
    settings(false);
    let client = start(&config, &projects, &projects).await;
    call(&client, "project_open", json!({ "path": "tio2.mwproj" })).await;
    call(&client, "structure_open", json!({ "structure": 1 })).await;
    let edit = json!({ "structure": 1, "values": [{ "parameter": "a", "value": 4.7 }] });
    let Value::Object(arguments) = edit else {
        unreachable!()
    };
    let params = CallToolRequestParams::new("structure_set_cell").with_arguments(arguments);
    let refused = client.call_tool(params.clone()).await.unwrap();
    assert_eq!(refused.is_error, Some(true));
    let text = serde_json::to_string(&refused.content).unwrap();
    assert!(text.contains("not_permitted"), "{text}");

    // The user allows it; the next call may.
    settings(true);
    let before = fs::read(config.join("settings.json")).unwrap();
    let session = fs::read(config.join("state.json")).unwrap();
    let edited = client.call_tool(params).await.unwrap();
    assert_ne!(edited.is_error, Some(true), "{edited:#?}");
    client.cancel().await.unwrap();
    let rutile = fs::read_to_string(tio2.join(format!("structures/{RUTILE}.json"))).unwrap();
    assert!(rutile.contains("4.7"), "{rutile}");
    // Both calls are in the project's audit log; the refusal too.
    let log = fs::read_to_string(tio2.join("audit.jsonl")).unwrap();
    let edits = log.matches(r#""capability":"structure.set_cell""#).count();
    assert_eq!(edits, 2, "{log}");
    assert_eq!(fs::read(config.join("settings.json")).unwrap(), before);
    assert_eq!(fs::read(config.join("state.json")).unwrap(), session);
}

/// The client of `materials-workbench mcp`, with the configuration
/// directory `config` and the directory `allow`, started in `cwd`.
async fn start(config: &Path, allow: &Path, cwd: &Path) -> Client {
    let command = Command::new(APP).configure(|command| {
        command
            .arg("mcp")
            .arg("--allow")
            .arg(allow)
            .env("MW_CONFIG_DIR", config)
            .current_dir(cwd);
    });
    let transport = TokioChildProcess::new(command).expect("the server starts");
    ().serve(transport).await.expect("initialize")
}

async fn tools(client: &Client) -> Vec<String> {
    let tools = client.list_all_tools().await.unwrap();
    tools.into_iter().map(|t| t.name.to_string()).collect()
}

/// Runs the calls of the application's servers on a host of their own, as
/// the application does on its main thread. The host opens what the first
/// call's client allows, as the application would for that client.
fn host_thread(mut calls: mw_mcp::Calls) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut host = None;
            while let Some(call) = calls.next().await {
                let host = host.get_or_insert_with(|| mw_mcp::Headless::new(call.access().clone()));
                call.run(host);
            }
        });
    })
}

/// Milestone M12: with the application listening, the command relays the
/// client to it; the application serves the tools it has with a window.
#[tokio::test]
async fn relays_to_the_running_application() {
    let temp = tempfile::tempdir().unwrap();
    let projects = temp.path().join("projects");
    fs::create_dir(&projects).unwrap();
    mw_project_file::samples::install(&projects).unwrap();
    let config = temp.path().join("config");
    // Where the application listens with this configuration directory.
    let endpoint = mw_mcp::Endpoint::in_dir(config.join("run"));
    let (dispatcher, calls) = mw_mcp::dispatcher();
    let listening = mw_mcp::listen(&endpoint, dispatcher).unwrap();
    let host = host_thread(calls);

    // Started in the projects' directory: relative paths are taken from
    // there, not from the application's working directory.
    let client = start(&config, &projects, &projects).await;
    let info = client.peer_info().expect("server info");
    let instructions = info.instructions.as_deref().unwrap();
    assert!(
        instructions.contains("connected to the application"),
        "{instructions}"
    );
    let served = tools(&client).await;
    assert!(served.contains(&"project_open".to_string()));
    assert!(!served.contains(&"file_open".to_string()));

    let project = call(&client, "project_open", json!({ "path": "tio2.mwproj" })).await;
    assert_eq!(project["name"], "TiO₂ 相研究");
    let id = project["structures"][0]["structure"].clone();
    call(&client, "structure_open", json!({ "structure": id })).await;
    let summary = call(&client, "structure_summary", json!({ "structure": id })).await;
    assert_eq!(summary["formula"], "O2Ti");
    let uri = format!("mw://project/{TIO2}/structure/{RUTILE}.cif");
    let read = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .unwrap();
    let cif = serde_json::to_value(&read.contents[0]).unwrap();
    assert!(cif["text"].as_str().unwrap().contains("data_rutile"));
    client.cancel().await.unwrap();

    // The application stops listening: the next client runs without it.
    drop(listening);
    host.join().unwrap();
    let client = start(&config, &projects, &projects).await;
    assert!(tools(&client).await.contains(&"file_open".to_string()));
    client.cancel().await.unwrap();
}

/// An application that did not quit cleanly leaves its socket and token
/// behind; the command runs without a window then.
#[cfg(unix)]
#[tokio::test]
async fn a_stale_socket_is_no_application() {
    use std::os::unix::fs::PermissionsExt as _;
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config");
    let run = config.join("run");
    fs::create_dir_all(&run).unwrap();
    fs::set_permissions(&run, fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = mw_mcp::Endpoint::in_dir(&run);
    drop(std::os::unix::net::UnixListener::bind(endpoint.socket()).unwrap());
    fs::write(endpoint.token_file(), "0".repeat(64)).unwrap();
    let client = start(&config, temp.path(), temp.path()).await;
    assert!(tools(&client).await.contains(&"file_open".to_string()));
    let list = call(&client, "project_list", json!({})).await;
    assert_eq!(list["open"], json!([]));
    client.cancel().await.unwrap();
}

#[test]
fn help_without_a_window() {
    let output = StdCommand::new(APP)
        .args(["mcp", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.starts_with("Usage: materials-workbench mcp [--allow DIR]..."));
    let output = StdCommand::new(APP)
        .args(["mcp", "--write"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
