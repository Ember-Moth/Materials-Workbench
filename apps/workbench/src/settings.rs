//! Appearance, language and MCP clients: the settings dialog and the
//! commands behind it. Choices are saved as they are made; appearance and
//! language are applied at the next start (`config.rs`), MCP clients and
//! what AI clients may do at once (`mcp.rs`, `host.rs`).

use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme, Sizable, Theme, ThemeMode, WindowExt, h_flex, locale, set_locale, v_flex,
};
use gpui_kit::*;
use mw_automation::{Client, Permissions};

use crate::config::{AiSettings, AppPaths, ClientSetting, Settings};

/// Interface languages, in the order the settings dialog lists them.
const LANGUAGES: [(&str, &str); 2] = [("zh-CN", "简体中文"), ("en", "English")];

fn mode(dark: bool) -> ThemeMode {
    if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}

/// Whether MCP clients may connect to the application, as the settings
/// say. The workbench listens for them while it is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct McpSetting(pub bool);

impl Global for McpSetting {}

impl McpSetting {
    pub fn get(cx: &App) -> bool {
        cx.try_global::<Self>().is_some_and(|setting| setting.0)
    }
}

/// What AI clients may do (`docs/design/ai-integration.md` §7.2), as the
/// settings say.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AiSetting(pub AiSettings);

impl Global for AiSetting {}

impl AiSetting {
    pub fn get(cx: &App) -> AiSettings {
        cx.try_global::<Self>()
            .map(|setting| setting.0.clone())
            .unwrap_or_default()
    }

    /// What `client` may do now.
    pub fn permissions(client: &Client, cx: &App) -> Permissions {
        cx.try_global::<Self>()
            .map_or_else(Permissions::default, |setting| {
                setting.0.permissions(client)
            })
    }

    /// Remembers a client that connected, so that the settings list it.
    pub fn remember(client: &Client, cx: &mut App) {
        let mut ai = Self::get(cx);
        match ai.clients.iter_mut().find(|c| c.name == client.name) {
            Some(known) if known.title == client.title => return,
            Some(known) => known.title = client.title.clone(),
            None => ai.clients.push(ClientSetting {
                name: client.name.clone(),
                title: client.title.clone(),
                modify_data: None,
                write_files: None,
            }),
        }
        set_ai(ai, cx);
    }
}

/// Applies the saved settings; at start, before the window opens.
pub fn apply_saved(cx: &mut App) {
    let settings = Settings::load(cx.global::<AppPaths>());
    cx.set_global(McpSetting(settings.mcp));
    cx.set_global(AiSetting(settings.ai));
    let language = LANGUAGES
        .iter()
        .find(|(code, _)| *code == settings.language)
        .map_or(LANGUAGES[0].0, |(code, _)| code);
    set_locale(language);
    Theme::change(mode(settings.dark), None, cx);
}

/// Saves the current settings.
fn save(cx: &App) {
    let settings = Settings {
        dark: cx.theme().is_dark(),
        language: locale().to_string(),
        mcp: McpSetting::get(cx),
        ai: AiSetting::get(cx),
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

/// Lets MCP clients connect, or stops them; the workbench follows.
pub fn set_mcp(enabled: bool, cx: &mut App) {
    cx.set_global(McpSetting(enabled));
    cx.refresh_windows();
    save(cx);
}

/// Changes what AI clients may do; the next call follows.
pub fn set_ai(ai: AiSettings, cx: &mut App) {
    cx.set_global(AiSetting(ai));
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

/// A setting with an explanation under its name: the explanation is the
/// decision, who gets in and what they can do.
fn described(label: SharedString, description: SharedString, control: AnyElement, cx: &App) -> Div {
    h_flex()
        .items_start()
        .justify_between()
        .gap_4()
        .py_2()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(description),
                ),
        )
        .child(control)
}

/// One permission for all clients, or for one client.
#[derive(Clone, Copy)]
enum Permission {
    ModifyData,
    WriteFiles,
}

fn switch(
    id: impl Into<ElementId>,
    permission: Permission,
    client: Option<&str>,
    ai: &AiSettings,
) -> Switch {
    let own = client.and_then(|name| ai.clients.iter().find(|c| c.name == name));
    let checked = match (permission, own) {
        (Permission::ModifyData, Some(own)) => own.modify_data.unwrap_or(ai.modify_data),
        (Permission::WriteFiles, Some(own)) => own.write_files.unwrap_or(ai.write_files),
        (Permission::ModifyData, None) => ai.modify_data,
        (Permission::WriteFiles, None) => ai.write_files,
    };
    let client = client.map(str::to_owned);
    Switch::new(id)
        .checked(checked)
        .on_change(move |on, _, cx| {
            let mut ai = AiSetting::get(cx);
            let own = client
                .as_ref()
                .and_then(|name| ai.clients.iter_mut().find(|c| &c.name == name));
            match (permission, own) {
                (Permission::ModifyData, Some(own)) => own.modify_data = Some(*on),
                (Permission::WriteFiles, Some(own)) => own.write_files = Some(*on),
                (Permission::ModifyData, None) => ai.modify_data = *on,
                (Permission::WriteFiles, None) => ai.write_files = *on,
            }
            set_ai(ai, cx);
        })
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
        let ai = AiSetting::get(cx);
        let clients = (!ai.clients.is_empty()).then(|| {
            // The two columns line up under their headings.
            let cell = || h_flex().w_20().justify_center();
            v_flex()
                .gap_1()
                .py_2()
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(div().flex_1().child(tr!("settings.ai_clients")))
                        .child(cell().child(tr!("settings.ai_clients_modify")))
                        .child(cell().child(tr!("settings.ai_clients_write"))),
                )
                .children(ai.clients.iter().map(|client| {
                    let name = client.title.clone().unwrap_or_else(|| client.name.clone());
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().min_w_0().text_sm().truncate().child(name))
                        .child(
                            cell().child(
                                switch(
                                    SharedString::from(format!("settings-modify-{}", client.name)),
                                    Permission::ModifyData,
                                    Some(&client.name),
                                    &ai,
                                )
                                .small(),
                            ),
                        )
                        .child(
                            cell().child(
                                switch(
                                    SharedString::from(format!("settings-write-{}", client.name)),
                                    Permission::WriteFiles,
                                    Some(&client.name),
                                    &ai,
                                )
                                .small(),
                            ),
                        )
                }))
        });
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
                ))
                .child(described(
                    tr!("settings.mcp"),
                    tr!("settings.mcp_description"),
                    Switch::new("settings-mcp")
                        .checked(McpSetting::get(cx))
                        .on_change(|enabled, _, cx| set_mcp(*enabled, cx))
                        .into_any_element(),
                    cx,
                ))
                .child(described(
                    tr!("settings.ai_modify"),
                    tr!("settings.ai_modify_description"),
                    switch("settings-ai-modify", Permission::ModifyData, None, &ai)
                        .into_any_element(),
                    cx,
                ))
                .child(described(
                    tr!("settings.ai_write"),
                    tr!("settings.ai_write_description"),
                    switch("settings-ai-write", Permission::WriteFiles, None, &ai)
                        .into_any_element(),
                    cx,
                ))
                .children(clients),
        )
    });
}
