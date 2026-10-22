use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::command::CommandState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme, Icon, IconName, Sizable, StyledExt, WindowExt, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_project::{
    ImportedEntry, ProjectCatalog, ProjectSidebar, ProjectSidebarEvent, StructureEntryId,
    StructureSource,
};
use mw_structure::import::OpenedFile;
use mw_structure::{StructureDocument, StructureWorkspace};

use crate::actions::*;
use crate::{palette, settings};

/// The main window: project sidebar on the window background, and the open
/// structure (or an empty state) on a card beside it. Owns which structure
/// is open and routes window-wide commands.
pub struct Workbench {
    focus_handle: FocusHandle,
    sidebar: Entity<ProjectSidebar>,
    sidebar_open: bool,
    workspace: Option<Entity<StructureWorkspace>>,
    palette: Entity<CommandState>,
    _subscriptions: Vec<Subscription>,
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|_| ProjectSidebar::new(ProjectCatalog::sample()));
        let subscriptions = vec![cx.subscribe_in(&sidebar, window, Self::on_sidebar_event)];
        let first = sidebar.read(cx).catalog().first_structure();
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            sidebar,
            sidebar_open: true,
            workspace: None,
            palette: cx.new(|cx| CommandState::new(window, cx)),
            _subscriptions: subscriptions,
        };
        if let Some(id) = first {
            this.open_structure(id, window, cx);
        } else {
            this.focus_handle.focus(window, cx);
        }
        this
    }

    pub(crate) fn open_structure(
        &mut self,
        id: StructureEntryId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) =
            self.sidebar
                .read(cx)
                .catalog()
                .structure(id)
                .and_then(|(project, entry)| {
                    let (project, name) = (project.name.clone(), entry.name.clone());
                    match &entry.source {
                        StructureSource::Sample(key) => {
                            Some(StructureDocument::from_sample(project, name, *key))
                        }
                        StructureSource::Imported(block) => {
                            StructureDocument::from_import(project, name, block)
                        }
                    }
                })
        else {
            return;
        };
        let workspace = cx.new(|cx| StructureWorkspace::new(document, window, cx));
        let focus = workspace.focus_handle(cx);
        focus.focus(window, cx);
        self.workspace = Some(workspace);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_selected(Some(id), cx));
        cx.notify();
    }

    /// The open structure, if any; only the screenshot tour needs this so far.
    #[cfg(feature = "screenshot")]
    pub(crate) fn workspace(&self) -> Option<&Entity<StructureWorkspace>> {
        self.workspace.as_ref()
    }

    pub(crate) fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.sidebar_open = open;
        cx.notify();
    }

    /// Reads the files in the background, adds the structures they hold to
    /// the catalog, opens the first one and reports how each file went.
    pub(crate) fn import_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let read = cx.background_spawn(async move {
            paths.into_iter().map(OpenedFile::read).collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let files = read.await;
            this.update_in(cx, |this, window, cx| {
                this.add_opened_files(files, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn add_opened_files(
        &mut self,
        files: Vec<OpenedFile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project_name = tr!("project.imported").to_string();
        let mut first = None;
        for file in &files {
            let entries: Vec<ImportedEntry> = file
                .structures()
                .into_iter()
                .map(|(name, block)| ImportedEntry::new(name, file.file_name(), block))
                .collect();
            let ids = self.sidebar.update(cx, |sidebar, cx| {
                sidebar.add_imported(&project_name, entries, cx)
            });
            first = first.or(ids.first().copied());
        }
        if let Some(id) = first {
            self.open_structure(id, window, cx);
        }
        // After opening, so the notifications are not lost behind a focus
        // change and refer to what is now on screen.
        for file in &files {
            file.notify(window, cx);
        }
    }

    /// Shows the empty state; only the screenshot tour needs this so far.
    #[cfg(feature = "screenshot")]
    pub(crate) fn close_structure(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace = None;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_selected(None, cx));
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn on_sidebar_event(
        &mut self,
        _: &Entity<ProjectSidebar>,
        event: &ProjectSidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ProjectSidebarEvent::OpenStructure(id) => self.open_structure(*id, window, cx),
            ProjectSidebarEvent::NewProject => self.new_project(&NewProject, window, cx),
            ProjectSidebarEvent::OpenFile => self.open_file(&OpenFile, window, cx),
            ProjectSidebarEvent::SearchCommands => {
                self.toggle_command_palette(&ToggleCommandPalette, window, cx)
            }
            ProjectSidebarEvent::OpenSettings => self.open_settings(&OpenSettings, window, cx),
            ProjectSidebarEvent::Hide => self.toggle_sidebar(&ToggleSidebar, window, cx),
        }
    }

    /// Mock: tells the user a command exists but does nothing yet.
    fn not_implemented(&self, feature: SharedString, window: &mut Window, cx: &mut App) {
        window.push_notification(
            Notification::info(tr!("mock.not_implemented", feature = feature.to_string())),
            cx,
        );
    }

    // MARK: Actions

    pub(crate) fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
            return;
        }
        palette::open(self.palette.clone(), self.workspace.is_some(), window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.set_sidebar_open(!self.sidebar_open, cx);
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let paths = match paths.await {
                Ok(Ok(Some(paths))) => paths,
                // Cancelled.
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(error)) => {
                    let reason = error.to_string();
                    this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error(tr!("open.unavailable", reason = reason)),
                            cx,
                        )
                    })
                    .ok();
                    return;
                }
            };
            this.update_in(cx, |this, window, cx| this.import_paths(paths, window, cx))
                .ok();
        })
        .detach();
    }

    fn new_project(&mut self, _: &NewProject, window: &mut Window, cx: &mut Context<Self>) {
        self.not_implemented(tr!("feature.new_project"), window, cx);
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        settings::open(window, cx);
    }

    fn toggle_dark_mode(
        &mut self,
        _: &ToggleDarkMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        settings::toggle_dark_mode(window, cx);
    }

    fn switch_language(&mut self, _: &SwitchLanguage, _: &mut Window, cx: &mut Context<Self>) {
        settings::toggle_language(cx);
    }

    // MARK: Rendering

    fn render_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let sample = self.sidebar.read(cx).catalog().first_structure();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .p_8()
            .child(
                div()
                    .p_3()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::Inbox).large()),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap_1()
                    .max_w_96()
                    .child(div().text_base().font_semibold().child(tr!("empty.title")))
                    .child(
                        div()
                            .text_sm()
                            .text_center()
                            .text_color(theme.muted_foreground)
                            .child(tr!("empty.description")),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("empty-open-file")
                            .label(tr!("empty.open_file"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_file(&OpenFile, window, cx)
                            })),
                    )
                    .when_some(sample, |this, id| {
                        this.child(
                            Button::new("empty-open-sample")
                                .ghost()
                                .label(tr!("empty.open_sample"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_structure(id, window, cx)
                                })),
                        )
                    }),
            )
    }
}

impl Render for Workbench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.workspace {
            Some(workspace) => workspace.clone().into_any_element(),
            None => self.render_empty_state(cx).into_any_element(),
        };
        let theme = cx.theme();

        h_flex()
            .id("workbench")
            .key_context("Workbench")
            .track_focus(&self.focus_handle)
            .size_full()
            .items_stretch()
            .bg(theme.sidebar)
            .text_color(theme.foreground)
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::new_project))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::toggle_dark_mode))
            .on_action(cx.listener(Self::switch_language))
            // Files dropped anywhere on the window are opened.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.import_paths(paths.paths().to_vec(), window, cx)
            }))
            .when(self.sidebar_open, |this| this.child(self.sidebar.clone()))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .p_2()
                    .when(!self.sidebar_open, |this| {
                        // Room for the traffic lights, and the way back.
                        this.child(
                            h_flex().h_8().flex_none().pl_20().child(
                                Button::new("show-sidebar")
                                    .ghost()
                                    .small()
                                    .icon(IconName::PanelLeft)
                                    .tooltip_with_action(tr!("sidebar.show"), &ToggleSidebar, None)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_sidebar(&ToggleSidebar, window, cx)
                                    })),
                            ),
                        )
                    })
                    .child(
                        div()
                            .id("workbench-content")
                            .flex_1()
                            .min_h_0()
                            .rounded(theme.radius_lg)
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .overflow_hidden()
                            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                                style
                                    .border_color(cx.theme().drag_border)
                                    .bg(cx.theme().drop_target)
                            })
                            .child(content),
                    ),
            )
    }
}
