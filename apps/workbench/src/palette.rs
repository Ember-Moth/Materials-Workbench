//! The command palette (⌘K): every command of the window and of the open
//! structure, and the recent projects, searchable in Chinese, English or
//! pinyin.

use std::path::PathBuf;
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
    run: Run,
}

/// What confirming an entry does.
enum Run {
    Action(Box<dyn Action>),
    /// Opens a recent project.
    Recent(PathBuf),
}

impl Run {
    fn boxed_clone(&self) -> Self {
        match self {
            Self::Action(action) => Self::Action(action.boxed_clone()),
            Self::Recent(path) => Self::Recent(path.clone()),
        }
    }
}

/// Opens a recent project, for the palette's entries.
pub type OpenRecent = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;

fn shell_entries() -> Vec<Entry> {
    let entry = |label, keywords: &[&str], icon: Lucide, action: Box<dyn Action>| Entry {
        label,
        keywords: keywords
            .iter()
            .map(|k| SharedString::from(k.to_string()))
            .collect(),
        icon: Icon::new(icon),
        run: Run::Action(action),
    };
    vec![
        entry(
            tr!("command.save"),
            &["save", "baocun"],
            Lucide::Save,
            Box::new(SaveProject),
        ),
        entry(
            tr!("command.save_as"),
            &["save as", "lingcunwei"],
            Lucide::Save,
            Box::new(SaveProjectAs),
        ),
        entry(
            tr!("command.open_file"),
            &[
                "open file",
                "import",
                "cif",
                "poscar",
                "vasp",
                "contcar",
                "dakai",
            ],
            Lucide::FolderOpen,
            Box::new(OpenFile),
        ),
        entry(
            tr!("command.open_project"),
            &["open project", "dakai xiangmu"],
            Lucide::Folder,
            Box::new(OpenProject),
        ),
        entry(
            tr!("command.new_project"),
            &["new project", "xinjian xiangmu"],
            Lucide::Plus,
            Box::new(NewProject),
        ),
        entry(
            tr!("command.close_project"),
            &["close project", "guanbi xiangmu"],
            Lucide::FolderX,
            Box::new(CloseProject),
        ),
        entry(
            tr!("command.open_samples"),
            &["sample projects", "examples", "shili"],
            Lucide::BookOpen,
            Box::new(OpenSampleProjects),
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
        entry(
            tr!("command.export_ai_log"),
            &["ai log", "audit", "mcp", "export", "caozuo jilu", "daochu"],
            Lucide::FileClock,
            Box::new(ExportAiLog),
        ),
    ]
}

/// Opens the palette. A confirmed command is dispatched from the focus that
/// was current before the palette opened, so the views that own the command
/// receive it.
pub fn open(
    state: Entity<CommandState>,
    include_structure: bool,
    recent: Vec<(SharedString, PathBuf)>,
    open_recent: OpenRecent,
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
                run: Run::Action(c.action),
            })
            .collect();
        groups.push((tr!("command.group.structure"), structure));
    }
    groups.push((tr!("command.group.app"), shell_entries()));
    if !recent.is_empty() {
        let recent = recent
            .into_iter()
            .map(|(name, path)| Entry {
                label: tr!("command.open_recent", name = name.to_string()),
                keywords: vec![
                    "recent".into(),
                    "zuijin".into(),
                    path.display().to_string().into(),
                ],
                icon: Icon::new(Lucide::Clock),
                run: Run::Recent(path),
            })
            .collect();
        groups.push((tr!("command.group.recent"), recent));
    }

    let actions: Rc<Vec<Vec<Run>>> = Rc::new(
        groups
            .iter()
            .map(|(_, entries)| entries.iter().map(|e| e.run.boxed_clone()).collect())
            .collect(),
    );
    let groups = Rc::new(groups);

    window.open_dialog(cx, move |dialog, _, _| {
        let state = state.clone();
        let groups = groups.clone();
        let actions = actions.clone();
        let previous_focus = previous_focus.clone();
        let open_recent = open_recent.clone();
        dialog
            .close_button(false)
            .p_0()
            .content(move |content, _, _| {
                let mut command = Command::new(&state)
                    .bordered(false)
                    .placeholder(tr!("command.placeholder"));
                for (label, entries) in groups.iter() {
                    let items = entries.iter().map(|e| {
                        let item = CommandItem::new()
                            .label(e.label.clone())
                            .icon(e.icon.clone())
                            .keywords(e.keywords.clone());
                        match &e.run {
                            // Supplies the shortcut hint; the dispatch from
                            // inside the dialog reaches no handler,
                            // `on_confirm` below delivers it.
                            Run::Action(action) => item.action(action.boxed_clone()),
                            Run::Recent(_) => item,
                        }
                    });
                    command = command.group(CommandGroup::new().label(label.clone()).items(items));
                }
                let actions = actions.clone();
                let previous_focus = previous_focus.clone();
                let open_recent = open_recent.clone();
                content.child(
                    command
                        .on_confirm(move |ix, window, cx| {
                            let Some(run) = actions
                                .get(ix.section)
                                .and_then(|group| group.get(ix.row))
                                .map(Run::boxed_clone)
                            else {
                                return;
                            };
                            window.close_dialog(cx);
                            if let Some(focus) = &previous_focus {
                                focus.focus(window, cx);
                            }
                            let open_recent = open_recent.clone();
                            window.defer(cx, move |window, cx| match run {
                                Run::Action(action) => window.dispatch_action(action, cx),
                                Run::Recent(path) => open_recent(path, window, cx),
                            });
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
