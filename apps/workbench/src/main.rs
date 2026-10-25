//! Materials Workbench desktop application: the window shell that composes
//! the project sidebar and the structure workspace (`docs/architecture.md`).

// Release builds on Windows open no console window beside the application's.
// `materials-workbench mcp` still works: MCP clients start it with pipes,
// which the process inherits all the same (`docs/design/release.md` §4).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

rust_i18n::i18n!("locales", fallback = "en");

/// Looks up an application string in this crate's locale files.
macro_rules! tr {
    ($($args:tt)*) => {
        gpui_kit::SharedString::from(rust_i18n::t!($($args)*).to_string())
    };
}

mod actions;
mod ai_log;
mod assets;
mod config;
mod host;
mod launch;
mod mcp;
mod palette;
mod projects;
#[cfg(feature = "screenshot")]
mod screenshot;
mod settings;
#[cfg(test)]
mod ui_tests;
mod workbench;

use std::process::ExitCode;

use gpui_kit::component::TitleBar;
use gpui_kit::*;

use futures::StreamExt as _;

use config::AppPaths;
use launch::Launch;
use workbench::Workbench;

fn main() -> ExitCode {
    let requested = match launch::parse(std::env::args_os().skip(1)) {
        Ok(Launch::Open(paths)) => paths,
        // `materials-workbench mcp` serves MCP clients on stdio and opens no
        // window, so it starts before GPUI does.
        Ok(Launch::Mcp(args)) => return mcp::run(args),
        Ok(Launch::Help) => {
            print!("{}", launch::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(Launch::Version) => {
            println!("materials-workbench {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("materials-workbench: {message}\n\n{}", launch::USAGE);
            return ExitCode::from(2);
        }
    };
    env_logger::init();
    // Files the system asks the application to open: on macOS the ones
    // double-clicked in the Finder or dropped on the Dock icon, also those
    // the application is started for, before the window is there.
    let (requests, mut requested_later) = futures::channel::mpsc::unbounded();
    let application = gpui_kit::application().with_assets(assets::AppAssets);
    application.on_open_urls(move |urls| {
        let paths = launch::paths_of_urls(&urls);
        if !paths.is_empty() {
            requests.unbounded_send(paths).ok();
        }
    });
    application.run(move |cx| {
        {
            use gpui_kit::component as gpui_component;
            rust_i18n::extend!(gpui_component);
        }
        gpui_kit::init(cx);
        // The Windows taskbar groups the windows by this and matches them
        // with the installer's shortcuts.
        cx.set_app_identity(launch::APP_ID, launch::APP_NAME);
        // The screenshot tour starts from scratch in a directory of its
        // own, never from the user's projects and settings.
        #[cfg(feature = "screenshot")]
        let paths = screenshot::paths().unwrap_or_else(AppPaths::from_environment);
        #[cfg(not(feature = "screenshot"))]
        let paths = AppPaths::from_environment();
        cx.set_global(paths);
        settings::apply_saved(cx);

        actions::bind_keys(cx);
        mw_structure::bind_keys(cx);
        cx.on_window_closed(|cx, _| cx.quit()).detach();

        let titlebar = TitlebarOptions {
            // Lines the traffic lights up with the sidebar's first row.
            traffic_light_position: Some(point(px(14.), px(18.))),
            ..TitleBar::title_bar_options()
        };
        let options = WindowOptions {
            // The initial window size is a platform boundary, not layout.
            window_bounds: Some(WindowBounds::centered(size(px(1440.), px(900.)), cx)),
            titlebar: Some(titlebar),
            // Linux desktops find the window's icon and launcher by it.
            app_id: Some(launch::LINUX_APP_ID.into()),
            ..Default::default()
        };
        let (window, workbench) = gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Workbench::new(window, cx))
        })
        .expect("failed to open the main window");
        // Quitting and closing the window ask first when projects have
        // unsaved changes. ⌘Q is handled here rather than on the
        // workbench, so that it is not lost while focus is in a dialog.
        let quitting = workbench.clone();
        cx.on_action(move |_: &actions::Quit, cx| {
            let workbench = quitting.clone();
            window
                .update(cx, |_, window, cx| {
                    if workbench.update(cx, |w, cx| w.request_quit(window, cx)) {
                        cx.quit();
                    }
                })
                .ok();
        });
        let closing = workbench.clone();
        window
            .update(cx, |_, window, cx| {
                window.on_window_should_close(cx, move |window, cx| {
                    closing.update(cx, |w, cx| w.request_quit(window, cx))
                });
            })
            .ok();
        // The files in the arguments, then those the system sends while
        // the application runs; the workbench opens them after what the
        // last session left open.
        window
            .update(cx, |_, window, cx| {
                workbench.update(cx, |w, cx| w.open_paths(requested, window, cx))
            })
            .ok();
        let opening = workbench.clone();
        cx.spawn(async move |cx| {
            while let Some(paths) = requested_later.next().await {
                window
                    .update(cx, |_, window, cx| {
                        opening.update(cx, |w, cx| w.open_paths(paths, window, cx))
                    })
                    .ok();
            }
        })
        .detach();
        cx.activate(true);
        #[cfg(feature = "screenshot")]
        screenshot::start(window, workbench, cx);
    });
    ExitCode::SUCCESS
}
