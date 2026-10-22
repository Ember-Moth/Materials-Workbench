use gpui_kit::{App, KeyBinding};

gpui_kit::actions!(
    workbench,
    [
        Quit,
        ToggleCommandPalette,
        ToggleSidebar,
        OpenFile,
        NewProject,
        OpenSettings,
        ToggleDarkMode,
        SwitchLanguage,
    ]
);

/// Window-wide key bindings (`docs/design/ui-shell.md` §8).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-k", ToggleCommandPalette, None),
        KeyBinding::new("cmd-\\", ToggleSidebar, None),
        KeyBinding::new("cmd-o", OpenFile, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
    ]);
}
