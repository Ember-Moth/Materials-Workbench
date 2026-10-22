use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::sidebar::{Sidebar, SidebarFooter, SidebarMenu, SidebarMenuItem};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use gpui_kit::*;

use crate::{ImportedEntry, ProjectCatalog, StructureEntryId, StructureOrigin};

/// Requests from the sidebar; the window that hosts it decides what they do.
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectSidebarEvent {
    OpenStructure(StructureEntryId),
    NewProject,
    OpenFile,
    SearchCommands,
    OpenSettings,
    Hide,
}

/// Navigation over the project catalog. Owns which structure is selected; the
/// host keeps it in sync with the open document through
/// [`ProjectSidebar::set_selected`].
pub struct ProjectSidebar {
    catalog: ProjectCatalog,
    selected: Option<StructureEntryId>,
}

impl EventEmitter<ProjectSidebarEvent> for ProjectSidebar {}

impl ProjectSidebar {
    pub fn new(catalog: ProjectCatalog) -> Self {
        Self {
            catalog,
            selected: None,
        }
    }

    pub fn catalog(&self) -> &ProjectCatalog {
        &self.catalog
    }

    /// Adds imported structures to the catalog; see
    /// [`ProjectCatalog::add_imported`].
    pub fn add_imported(
        &mut self,
        project_name: &str,
        entries: Vec<ImportedEntry>,
        cx: &mut Context<Self>,
    ) -> Vec<StructureEntryId> {
        let ids = self.catalog.add_imported(project_name, entries);
        if !ids.is_empty() {
            cx.notify();
        }
        ids
    }

    pub fn set_selected(&mut self, selected: Option<StructureEntryId>, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.selected = selected;
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
        SidebarMenu::new()
            .child(
                SidebarMenuItem::new(tr!("sidebar.new_project"))
                    .icon(IconName::Plus)
                    .on_click(
                        cx.listener(|this, _, _, cx| {
                            this.emit(ProjectSidebarEvent::NewProject, cx)
                        }),
                    ),
            )
            .child(
                SidebarMenuItem::new(tr!("sidebar.open_file"))
                    .icon(IconName::FolderOpen)
                    .suffix(shortcut("cmd-o"))
                    .on_click(
                        cx.listener(|this, _, _, cx| this.emit(ProjectSidebarEvent::OpenFile, cx)),
                    ),
            )
            .child(
                SidebarMenuItem::new(tr!("sidebar.search_commands"))
                    .icon(IconName::Search)
                    .suffix(shortcut("cmd-k"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.emit(ProjectSidebarEvent::SearchCommands, cx)
                    })),
            )
    }

    fn render_projects(&self, cx: &mut Context<Self>) -> SidebarMenu {
        let mut menu = SidebarMenu::new();
        for project in self.catalog.projects() {
            let structures = project.structures.iter().map(|entry| {
                let id = entry.id;
                let item = SidebarMenuItem::new(entry.name.clone())
                    .active(self.selected == Some(id))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.emit(ProjectSidebarEvent::OpenStructure(id), cx)
                    }));
                if matches!(entry.origin, StructureOrigin::Derived { .. }) {
                    item.suffix(|_, cx| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(tr!("sidebar.derived"))
                    })
                } else {
                    item
                }
            });
            menu = menu.child(
                SidebarMenuItem::new(project.name.clone())
                    .icon(IconName::Folder)
                    .default_open(true)
                    .click_to_toggle(true)
                    .children(structures),
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
