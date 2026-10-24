//! MCP clients (`docs/design/ai-integration.md` §5.1). `materials-workbench
//! mcp` starts instead of the interface: it connects the client to the
//! running application when that listens, and otherwise serves it without a
//! window, listing the projects the application opened recently (read from
//! the configuration directory on every call, never written).
//!
//! The application listens while the setting is on (`settings.rs`): each
//! client gets a server whose calls run here, on the main thread, against
//! the open projects (`host.rs`), and asks the user here before a client
//! writes a file.

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::*;
use mw_automation::{Answer, Client, Confirmation};
use mw_mcp::{Calls, Listening, RecentProject};

use crate::config::{AppPaths, Session, Settings};
use crate::host::AppHost;
use crate::workbench::Workbench;

/// The first argument that starts the server instead of the interface.
pub const COMMAND: &str = "mcp";

/// Runs the command with the arguments after `mcp`; returns when the client
/// disconnects.
pub fn run(args: impl IntoIterator<Item = String>) -> ExitCode {
    let paths = AppPaths::from_environment();
    let endpoint = paths.mcp_endpoint();
    let settings = paths.clone();
    let permissions =
        move |client: &mw_automation::Client| Settings::load(&settings).ai.permissions(client);
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
        Box::new(permissions),
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

/// How long a confirmation waits for the user before it counts as no.
const ASK_TIMEOUT: Duration = Duration::from_secs(300);

/// Asks the user in the window whether a client may write a file
/// (`docs/design/ai-integration.md` §5.3). `answer` gets the first of: the
/// user's choice, the dialog closing, or no choice in time, which all but
/// the write button count as no.
pub(crate) fn ask(
    client: &Client,
    question: &Confirmation,
    answer: Answer,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let waiting = Rc::new(RefCell::new(Some(answer)));
    // Whether the answer was still waiting for this reply.
    let reply = move |yes: bool| match waiting.borrow_mut().take() {
        Some(answer) => {
            answer(yes);
            true
        }
        None => false,
    };
    let reply = Rc::new(reply);
    let who = client.display_name().to_string();
    // The path goes on a line of its own: a long one would otherwise break
    // the sentence around it.
    let (description, path, ok) = match question {
        Confirmation::WriteFile {
            structure,
            format,
            path,
            replaces,
            ..
        } => {
            let (structure, format) = (structure.as_str(), format.as_str());
            let path = SharedString::from(path.display().to_string());
            if *replaces {
                let description = tr!(
                    "mcp.confirm.replace",
                    structure = structure,
                    format = format
                );
                (description, path, tr!("mcp.confirm.replace_ok"))
            } else {
                let description = tr!("mcp.confirm.write", structure = structure, format = format);
                (description, path, tr!("mcp.confirm.write_ok"))
            }
        }
        // A question this version cannot put into words is not asked.
        _ => {
            reply(false);
            return;
        }
    };
    let title = tr!("mcp.confirm.title", client = who.as_str());
    let timed_out = tr!("mcp.confirm.timed_out", client = who.as_str());
    let dialog_reply = reply.clone();
    window.open_alert_dialog(cx, move |dialog, _, cx| {
        let (ok_reply, cancel_reply, close_reply) = (
            dialog_reply.clone(),
            dialog_reply.clone(),
            dialog_reply.clone(),
        );
        let timed_out = timed_out.clone();
        dialog
            .title(title.clone())
            .description(
                v_flex()
                    .gap_2()
                    .child(description.clone())
                    .child(div().text_color(cx.theme().foreground).child(path.clone())),
            )
            .ok_text(ok.clone())
            .cancel_text(tr!("mcp.confirm.cancel"))
            .show_cancel(true)
            .on_ok(move |_, window, cx| {
                if !ok_reply(true) {
                    window.push_notification(Notification::warning(timed_out.clone()), cx);
                }
                true
            })
            .on_cancel(move |_, _, _| {
                cancel_reply(false);
                true
            })
            .on_close(move |_, _, _| {
                close_reply(false);
            })
    });
    cx.spawn_in(window, async move |_, cx| {
        cx.background_executor().timer(ASK_TIMEOUT).await;
        reply(false);
    })
    .detach();
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
