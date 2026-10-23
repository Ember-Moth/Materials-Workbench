//! `materials-workbench mcp` as MCP clients start it (ai-integration design
//! §5.1): no window, the protocol on stdio, and the projects the
//! application opened recently, read from its configuration directory
//! (`MW_CONFIG_DIR`). The protocol itself is tested in `mw-mcp`.

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

async fn call(client: &RunningService<RoleClient, ()>, tool: &str, arguments: Value) -> Value {
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
