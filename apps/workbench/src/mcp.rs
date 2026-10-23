//! MCP clients (`docs/design/ai-integration.md` §5.1). `materials-workbench
//! mcp` starts instead of the interface: it connects the client to the
//! running application when that listens, and otherwise serves it without a
//! window, listing the projects the application opened recently (read from
//! the configuration directory on every call, never written).
//!
//! The application listens while the setting is on (`settings.rs`): each
//! client gets a server whose calls run here, on the main thread, against
//! the open projects (`host.rs`).

use std::process::ExitCode;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;
use mw_mcp::{Calls, Listening, RecentProject};

use crate::config::{AppPaths, Session};
use crate::host::AppHost;
use crate::workbench::Workbench;

/// The first argument that starts the server instead of the interface.
pub const COMMAND: &str = "mcp";

/// Runs the command with the arguments after `mcp`; returns when the client
/// disconnects.
pub fn run(args: impl IntoIterator<Item = String>) -> ExitCode {
    let paths = AppPaths::from_environment();
    let endpoint = paths.mcp_endpoint();
    let recent = move || {
        Session::load(&paths)
            .map(|session| session.recent)
            .unwrap_or_default()
            .into_iter()
            .map(|recent| RecentProject::new(recent.id, recent.name, recent.path, recent.opened_at))
            .collect()
    };
    mw_mcp::main(
        "materials-workbench mcp",
        args,
        Box::new(recent),
        Some(endpoint),
    )
}

/// The application listening for MCP clients. Dropping it stops listening
/// and closes their connections.
pub(crate) struct Serving {
    // Dropped first: the listener's thread then has no task of the main
    // thread's left to wake as it stops.
    _calls: Task<()>,
    _listening: Listening,
}

/// Runs the calls servers hand over on the workbench, one at a time, until
/// every server is gone.
pub(crate) fn run_calls(
    mut calls: Calls,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Task<()> {
    cx.spawn_in(window, async move |this, cx| {
        while let Some(call) = calls.next().await {
            let access = call.access().clone();
            // A call that cannot run (the window is gone) is dropped, and its
            // server answers with an error.
            let ran = this.update_in(cx, |workbench, window, cx| {
                call.run(&mut AppHost::new(workbench, &access, window, cx))
            });
            if ran.is_err() {
                break;
            }
        }
    })
}

impl Workbench {
    /// Listens for MCP clients while the setting is on, and stops when it
    /// goes off.
    pub(crate) fn sync_mcp(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = crate::settings::McpSetting::get(cx);
        if !enabled {
            self.mcp = None;
            return;
        }
        if self.mcp.is_some() {
            return;
        }
        let endpoint = cx.global::<AppPaths>().mcp_endpoint();
        let (dispatcher, calls) = mw_mcp::dispatcher();
        match mw_mcp::listen(&endpoint, dispatcher) {
            Ok(listening) => {
                self.mcp = Some(Serving {
                    _calls: run_calls(calls, window, cx),
                    _listening: listening,
                });
            }
            Err(error) => {
                log::error!("could not listen for MCP clients: {error}");
                window.push_notification(
                    Notification::error(tr!("mcp.listen_failed", reason = error.to_string())),
                    cx,
                );
            }
        }
    }
}
