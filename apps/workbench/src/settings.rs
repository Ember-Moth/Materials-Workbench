//! Appearance and language: the settings dialog and the commands behind it.
//! Choices are saved as they are made and applied at the next start
//! (`config.rs`).

use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme, Theme, ThemeMode, WindowExt, h_flex, locale, set_locale, v_flex,
};
use gpui_kit::*;

use crate::config::{AppPaths, Settings};

/// Interface languages, in the order the settings dialog lists them.
const LANGUAGES: [(&str, &str); 2] = [("zh-CN", "简体中文"), ("en", "English")];

fn mode(dark: bool) -> ThemeMode {
    if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}

/// Applies the saved settings; at start, before the window opens.
pub fn apply_saved(cx: &mut App) {
    let settings = Settings::load(cx.global::<AppPaths>());
    let language = LANGUAGES
        .iter()
        .find(|(code, _)| *code == settings.language)
        .map_or(LANGUAGES[0].0, |(code, _)| code);
    set_locale(language);
    Theme::change(mode(settings.dark), None, cx);
}

/// Saves the current appearance and language.
fn save(cx: &App) {
    let settings = Settings {
        dark: cx.theme().is_dark(),
        language: locale().to_string(),
        ..Settings::default()
    };
    settings.save(cx.global::<AppPaths>());
}

pub fn set_dark_mode(dark: bool, window: &mut Window, cx: &mut App) {
    Theme::change(mode(dark), Some(window), cx);
    cx.refresh_windows();
    save(cx);
}

pub fn toggle_dark_mode(window: &mut Window, cx: &mut App) {
    set_dark_mode(!cx.theme().is_dark(), window, cx);
}

pub fn set_language(code: &str, cx: &mut App) {
    set_locale(code);
    cx.refresh_windows();
    save(cx);
}

/// Switches between the two interface languages.
pub fn toggle_language(cx: &mut App) {
    let next = if locale().to_string() == LANGUAGES[0].0 {
        LANGUAGES[1].0
    } else {
        LANGUAGES[0].0
    };
    set_language(next, cx);
}

pub fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, _, cx| {
        let theme = cx.theme();
        let current = locale().to_string();
        let language = LANGUAGES
            .iter()
            .position(|(code, _)| *code == current)
            .unwrap_or(0);
        let row = |label: SharedString, control: AnyElement| {
            h_flex()
                .justify_between()
                .gap_4()
                .py_2()
                .child(div().text_sm().child(label))
                .child(control)
        };
        dialog.title(tr!("settings.title")).w_96().child(
            v_flex()
                .gap_1()
                .child(row(
                    tr!("settings.dark_mode"),
                    Switch::new("settings-dark-mode")
                        .checked(theme.is_dark())
                        .on_change(|dark, window, cx| set_dark_mode(*dark, window, cx))
                        .into_any_element(),
                ))
                .child(row(
                    tr!("settings.language"),
                    RadioGroup::horizontal("settings-language")
                        .children(LANGUAGES.map(|(_, name)| name))
                        .selected_index(Some(language))
                        .on_change(|ix, _, cx| {
                            if let Some((code, _)) = LANGUAGES.get(*ix) {
                                set_language(code, cx);
                            }
                        })
                        .into_any_element(),
                )),
        )
    });
}
