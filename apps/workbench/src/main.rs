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
mod palette;
#[cfg(feature = "screenshot")]
mod screenshot;
mod settings;
mod workbench;

use gpui_kit::component::{TitleBar, set_locale};
use gpui_kit::*;

use workbench::Workbench;

/// The interface language until there is a saved preference.
const DEFAULT_LOCALE: &str = "zh-CN";

fn main() {
    env_logger::init();
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx| {
            {
                use gpui_kit::component as gpui_component;
                rust_i18n::extend!(gpui_component);
            }
            gpui_kit::init(cx);
            set_locale(DEFAULT_LOCALE);

            actions::bind_keys(cx);
            mw_structure::bind_keys(cx);
            cx.on_action(|_: &actions::Quit, cx| cx.quit());
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
            let (_window, _workbench) = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| Workbench::new(window, cx))
            })
            .expect("failed to open the main window");
            cx.activate(true);
            #[cfg(feature = "screenshot")]
            screenshot::start(_window, _workbench, cx);
        });
}
