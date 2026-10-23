use std::collections::HashSet;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::sidebar::{Sidebar, SidebarFooter, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{ProjectCatalog, ProjectId, StructureEntryId};

/// Requests from the sidebar; the window that hosts it decides what they do.
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectSidebarEvent {
    OpenStructure(StructureEntryId),
    NewProject,
    OpenProject,
    OpenFile,
    SearchCommands,
    OpenSettings,
    Hide,
    SaveProject(ProjectId),
    SaveProjectAs(ProjectId),
    RevealProject(ProjectId),
    CloseProject(ProjectId),
}

/// Navigation over the open projects. Owns the catalog and which structure
/// is selected; the host keeps the selection in sync with the open document
/// through [`ProjectSidebar::set_selected`], and tells it which structures
/// have unsaved edits.
pub struct ProjectSidebar {
    catalog: ProjectCatalog,
    selected: Option<StructureEntryId>,
    modified: HashSet<StructureEntryId>,
}

impl EventEmitter<ProjectSidebarEvent> for ProjectSidebar {}

impl ProjectSidebar {
    pub fn new(catalog: ProjectCatalog) -> Self {
        Self {
            catalog,
            selected: None,
            modified: HashSet::new(),
        }
    }

    pub fn catalog(&self) -> &ProjectCatalog {
        &self.catalog
    }

    /// Changes the catalog and redraws.
    pub fn update_catalog<R>(
        &mut self,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut ProjectCatalog) -> R,
    ) -> R {
        let result = change(&mut self.catalog);
        cx.notify();
        result
    }

    pub fn set_selected(&mut self, selected: Option<StructureEntryId>, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.selected = selected;
            cx.notify();
        }
    }

    /// The open structures whose documents have unsaved edits.
    pub fn set_modified(&mut self, modified: HashSet<StructureEntryId>, cx: &mut Context<Self>) {
        if self.modified != modified {
            self.modified = modified;
            cx.notify();
        }
    }

    fn emit(&mut self, event: ProjectSidebarEvent, cx: &mut Context<Self>) {
        cx.emit(event);
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The window's traffic lights sit in this row on macOS.
        h_flex().w_full().h_6().justify_end().child(
            Button::new("hide-sidebar")
                .ghost()
                .small()
                .icon(IconName::PanelLeft)
                .tooltip(tr!("sidebar.hide"))
                .on_click(cx.listener(|this, _, _, cx| this.emit(ProjectSidebarEvent::Hide, cx))),
        )
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> SidebarMenu {
        let shortcut = |keys: &str| {
            let keystroke = Keystroke::parse(keys).ok();
            move |_: &mut Window, _: &mut App| div().children(keystroke.clone().map(Kbd::new))
        };
        let item = |label: SharedString, icon: IconName, event: ProjectSidebarEvent| {
            SidebarMenuItem::new(label)
                .icon(icon)
                .on_click(cx.listener(move |this, _, _, cx| this.emit(event.clone(), cx)))
        };
        SidebarMenu::new()
            .child(item(
                tr!("sidebar.new_project"),
                IconName::Plus,
                ProjectSidebarEvent::NewProject,
            ))
            .child(
                item(
                    tr!("sidebar.open_project"),
                    IconName::Folder,
                    ProjectSidebarEvent::OpenProject,
                )
                .suffix(shortcut("cmd-shift-o")),
            )
            .child(
                item(
                    tr!("sidebar.open_file"),
                    IconName::FolderOpen,
                    ProjectSidebarEvent::OpenFile,
                )
                .suffix(shortcut("cmd-o")),
            )
            .child(
                item(
                    tr!("sidebar.search_commands"),
                    IconName::Search,
                    ProjectSidebarEvent::SearchCommands,
                )
                .suffix(shortcut("cmd-k")),
            )
    }

    fn render_projects(&self, cx: &mut Context<Self>) -> SidebarMenu {
        let mut menu = SidebarMenu::new();
        let sidebar = cx.entity().downgrade();
        for project in self.catalog.projects() {
            let structures = project.structures().iter().map(|entry| {
                let id = entry.id();
                let derived = entry.is_derived();
                // New structures are not in the project directory yet.
                let modified = self.modified.contains(&id) || entry.pending().is_some();
                SidebarMenuItem::new(SharedString::from(entry.name().to_string()))
                    .active(self.selected == Some(id))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.emit(ProjectSidebarEvent::OpenStructure(id), cx)
                    }))
                    .when(derived || modified, |item| {
                        item.suffix(move |_, cx| {
                            let theme = cx.theme();
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .when(derived, |this| this.child(tr!("sidebar.derived")))
                                .when(modified, |this| {
                                    this.child(
                                        div().id(("unsaved", id.0 as usize)).child("●").tooltip(
                                            |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(tr!(
                                                    "sidebar.unsaved"
                                                ))
                                                .build(window, cx)
                                            },
                                        ),
                                    )
                                })
                        })
                    })
            });
            let id = project.id();
            let saved = project.path().is_some();
            let sidebar = sidebar.clone();
            menu = menu.child(
                SidebarMenuItem::new(SharedString::from(project.name().to_string()))
                    .icon(IconName::Folder)
                    .default_open(true)
                    .click_to_toggle(true)
                    .children(structures)
                    .context_menu(move |menu, _, _| {
                        let item = |label: SharedString, event: ProjectSidebarEvent| {
                            let sidebar = sidebar.clone();
                            PopupMenuItem::new(label).on_click(move |_, _, cx| {
                                sidebar
                                    .update(cx, |this, cx| this.emit(event.clone(), cx))
                                    .ok();
                            })
                        };
                        menu.item(item(
                            tr!("sidebar.save"),
                            ProjectSidebarEvent::SaveProject(id),
                        ))
                        .item(item(
                            tr!("sidebar.save_as"),
                            ProjectSidebarEvent::SaveProjectAs(id),
                        ))
                        .item(
                            item(
                                tr!("sidebar.reveal"),
                                ProjectSidebarEvent::RevealProject(id),
                            )
                            .disabled(!saved),
                        )
                        .separator()
                        .item(item(
                            tr!("sidebar.close_project"),
                            ProjectSidebarEvent::CloseProject(id),
                        ))
                    }),
            );
        }
        menu
    }
}

impl Render for ProjectSidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        Sidebar::new("project-sidebar")
            .header(self.render_header(cx))
            .child(self.render_actions(cx))
            .child(self.render_projects(cx))
            .footer(
                SidebarFooter::new()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Icon::new(IconName::Settings))
                            .child(tr!("sidebar.settings")),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.emit(ProjectSidebarEvent::OpenSettings, cx)
                        }),
                    ),
            )
    }
}
