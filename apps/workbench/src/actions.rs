use gpui_kit::{App, KeyBinding};

gpui_kit::actions!(
    workbench,
    [
        Quit,
        ToggleCommandPalette,
        ToggleSidebar,
        OpenFile,
        NewProject,
        OpenProject,
        SaveProject,
        SaveProjectAs,
        CloseProject,
        OpenSampleProjects,
        OpenSettings,
        ExportAiLog,
        ToggleDarkMode,
        SwitchLanguage,
    ]
);

/// Window-wide key bindings (`docs/design/ui-shell.md` §8). `secondary` is
/// ⌘ on macOS and Ctrl elsewhere.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-k", ToggleCommandPalette, None),
        KeyBinding::new("secondary-\\", ToggleSidebar, None),
        KeyBinding::new("secondary-o", OpenFile, None),
        KeyBinding::new("secondary-shift-o", OpenProject, None),
        KeyBinding::new("secondary-s", SaveProject, None),
        KeyBinding::new("secondary-shift-s", SaveProjectAs, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
    ]);
}
