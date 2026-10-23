//! Materials Workbench desktop application: the window shell that composes
//! the project sidebar and the structure workspace (`docs/architecture.md`).

rust_i18n::i18n!("locales", fallback = "en");

/// Looks up an application string in this crate's locale files.
macro_rules! tr {
    ($($args:tt)*) => {
        gpui_kit::SharedString::from(rust_i18n::t!($($args)*).to_string())
    };
}

mod actions;
mod assets;
mod config;
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

use config::AppPaths;
use workbench::Workbench;

fn main() -> ExitCode {
    // `materials-workbench mcp` serves MCP clients on stdio and opens no
    // window, so it starts before GPUI does.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(mcp::COMMAND) {
        return mcp::run(args);
    }
    env_logger::init();
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx| {
            {
                use gpui_kit::component as gpui_component;
                rust_i18n::extend!(gpui_component);
            }
            gpui_kit::init(cx);
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
            cx.activate(true);
            #[cfg(feature = "screenshot")]
            screenshot::start(window, workbench, cx);
        });
    ExitCode::SUCCESS
}
