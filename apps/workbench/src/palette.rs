//! The command palette (⌘K): every command of the window and of the open
//! structure, searchable in Chinese, English or pinyin.

use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::{ActiveTheme, Icon, WindowExt, h_flex};
use gpui_kit::*;

use crate::actions::*;

struct Entry {
    label: SharedString,
    keywords: Vec<SharedString>,
    icon: Icon,
    action: Box<dyn Action>,
}

fn shell_entries() -> Vec<Entry> {
    let entry = |label, keywords: &[&str], icon: Lucide, action: Box<dyn Action>| Entry {
        label,
        keywords: keywords
            .iter()
            .map(|k| SharedString::from(k.to_string()))
            .collect(),
        icon: Icon::new(icon),
        action,
    };
    vec![
        entry(
            tr!("command.open_file"),
            &["open file", "import", "cif", "dakai"],
            Lucide::FolderOpen,
            Box::new(OpenFile),
        ),
        entry(
            tr!("command.new_project"),
            &["new project", "xinjian"],
            Lucide::Plus,
            Box::new(NewProject),
        ),
        entry(
            tr!("command.toggle_sidebar"),
            &["sidebar", "bianlan"],
            Lucide::PanelLeft,
            Box::new(ToggleSidebar),
        ),
        entry(
            tr!("command.toggle_dark_mode"),
            &["dark mode", "theme", "shense", "zhuti"],
            Lucide::Moon,
            Box::new(ToggleDarkMode),
        ),
        entry(
            tr!("command.switch_language"),
            &["language", "english", "chinese", "yuyan"],
            Lucide::Languages,
            Box::new(SwitchLanguage),
        ),
        entry(
            tr!("command.settings"),
            &["settings", "preferences", "shezhi"],
            Lucide::Settings,
            Box::new(OpenSettings),
        ),
    ]
}

/// Opens the palette. A confirmed command is dispatched from the focus that
/// was current before the palette opened, so the views that own the command
/// receive it.
pub fn open(
    state: Entity<CommandState>,
    include_structure: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let previous_focus = window.focused(cx);
    let mut groups: Vec<(SharedString, Vec<Entry>)> = Vec::new();
    if include_structure {
        let structure = mw_structure::palette_commands()
            .into_iter()
            .map(|c| Entry {
                label: c.label,
                keywords: c.keywords,
                icon: c.icon,
                action: c.action,
            })
            .collect();
        groups.push((tr!("command.group.structure"), structure));
    }
    groups.push((tr!("command.group.app"), shell_entries()));

    let actions: Rc<Vec<Vec<Box<dyn Action>>>> = Rc::new(
        groups
            .iter()
            .map(|(_, entries)| entries.iter().map(|e| e.action.boxed_clone()).collect())
            .collect(),
    );
    let groups = Rc::new(groups);

    window.open_dialog(cx, move |dialog, _, _| {
        let state = state.clone();
        let groups = groups.clone();
        let actions = actions.clone();
        let previous_focus = previous_focus.clone();
        dialog
            .close_button(false)
            .p_0()
            .content(move |content, _, _| {
                let mut command = Command::new(&state)
                    .bordered(false)
                    .placeholder(tr!("command.placeholder"));
                for (label, entries) in groups.iter() {
                    let items = entries.iter().map(|e| {
                        CommandItem::new()
                            .label(e.label.clone())
                            .icon(e.icon.clone())
                            .keywords(e.keywords.clone())
                            // Supplies the shortcut hint; the dispatch from inside
                            // the dialog reaches no handler, `on_confirm` below
                            // delivers it.
                            .action(e.action.boxed_clone())
                    });
                    command = command.group(CommandGroup::new().label(label.clone()).items(items));
                }
                let actions = actions.clone();
                let previous_focus = previous_focus.clone();
                content.child(
                    command
                        .on_confirm(move |ix, window, cx| {
                            let Some(action) = actions
                                .get(ix.section)
                                .and_then(|group| group.get(ix.row))
                                .map(|a| a.boxed_clone())
                            else {
                                return;
                            };
                            window.close_dialog(cx);
                            if let Some(focus) = &previous_focus {
                                focus.focus(window, cx);
                            }
                            window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
                        })
                        .footer(|_, _, cx| {
                            h_flex()
                                .gap_4()
                                .px_3()
                                .py_2()
                                .border_t_1()
                                .border_color(cx.theme().border)
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(tr!("command.footer.navigate"))
                                .child(tr!("command.footer.confirm"))
                                .child(tr!("command.footer.search"))
                        }),
                )
            })
    });
}
