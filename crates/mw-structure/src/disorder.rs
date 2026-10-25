//! Disorder (crystallography design §4.5, §7.2; `docs/design/ui-shell.md`
//! §4.2, §4.3, §5). The properties panel's disorder fields put the selected
//! sites in an assembly and group, or take them out of disorder, as one
//! `SetDisorder` step. The toolbar's popover chooses which group of each
//! assembly the viewport shows: view state, like the display range.

use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::input::InputState;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme, Selectable, Sizable, StyledExt, WindowExt, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mw_crystal::{Disorder, Edit, ShownGroups, SiteDisorder, SiteId, disorder_assemblies};

use crate::actions::EditDisorderGroups;
use crate::workspace::StructureWorkspace;

/// "A·2": a site's assembly and group as the interface writes them; "2"
/// without an assembly.
pub(crate) fn disorder_code(disorder: &Disorder) -> String {
    match &disorder.assembly {
        Some(assembly) => format!("{assembly}·{}", disorder.group),
        None => disorder.group.clone(),
    }
}

/// Whether a group is negative, as SHELX marks a site that is an
/// alternative to its own images; read, not interpreted (design §7.2).
fn is_negative(disorder: &Disorder) -> bool {
    disorder.group.parse::<i64>().is_ok_and(|group| group < 0)
}

/// The fields of the properties panel that edit the selected sites'
/// disorder.
pub(crate) struct DisorderFields {
    pub assembly: Entity<InputState>,
    pub group: Entity<InputState>,
    /// The assembly and group the fields were last set to show. Only typed
    /// values that differ are committed, so passing through the empty
    /// fields of sites with different values changes nothing.
    shown: (String, String),
}

impl DisorderFields {
    pub(crate) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            assembly: cx.new(|cx| InputState::new(window, cx)),
            group: cx.new(|cx| InputState::new(window, cx)),
            shown: (String::new(), String::new()),
        }
    }

    /// The assembly and group typed, without surrounding spaces.
    pub(crate) fn typed(&self, cx: &App) -> (String, String) {
        let text = |state: &Entity<InputState>| state.read(cx).value().trim().to_string();
        (text(&self.assembly), text(&self.group))
    }
}

/// What the selected sites' disorder is.
pub(crate) enum CommonDisorder {
    /// All have this one, or none.
    Same(Option<Disorder>),
    Different,
}

impl StructureWorkspace {
    /// The sites the disorder fields edit: every site of the selection,
    /// including those of single atoms selected, since the panel edits
    /// sites (design §7.2).
    pub(crate) fn disorder_sites(&self) -> Vec<SiteId> {
        let structure = self.document.structure();
        self.selection
            .sites()
            .into_iter()
            .filter(|&site| structure.site(site).is_some())
            .collect()
    }

    pub(crate) fn common_disorder(&self) -> CommonDisorder {
        let structure = self.document.structure();
        let mut values = self
            .disorder_sites()
            .into_iter()
            .filter_map(|site| structure.site(site))
            .map(|site| site.disorder.clone());
        let Some(first) = values.next() else {
            return CommonDisorder::Same(None);
        };
        if values.all(|value| value == first) {
            CommonDisorder::Same(first)
        } else {
            CommonDisorder::Different
        }
    }

    /// Shows the selected sites' disorder in the fields: their value when
    /// they share one, otherwise nothing with "Several values".
    pub(crate) fn sync_disorder_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (assembly, group, placeholder) = match self.common_disorder() {
            CommonDisorder::Same(Some(disorder)) => (
                disorder.assembly.unwrap_or_default(),
                disorder.group,
                SharedString::default(),
            ),
            CommonDisorder::Same(None) => (String::new(), String::new(), SharedString::default()),
            CommonDisorder::Different => (
                String::new(),
                String::new(),
                tr!("panel.site.disorder_several"),
            ),
        };
        let fields = &self.disorder;
        for (state, text) in [(&fields.assembly, &assembly), (&fields.group, &group)] {
            let (text, placeholder) = (text.clone(), placeholder.clone());
            state.update(cx, |state, cx| {
                state.set_value(text, window, cx);
                state.set_placeholder(placeholder, window, cx);
            });
        }
        self.disorder.shown = (assembly, group);
    }

    /// Enter in a disorder field, or leaving it: a group puts the selected
    /// sites in it, within the typed assembly if any; neither takes them
    /// out of disorder. An assembly without a group waits for the group.
    pub(crate) fn commit_disorder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (assembly, group) = self.disorder.typed(cx);
        if (&assembly, &group) == (&self.disorder.shown.0, &self.disorder.shown.1) {
            return;
        }
        let disorder = match (assembly.is_empty(), group.is_empty()) {
            (true, true) => None,
            // The model has no assembly without a group; the panel says so.
            (false, true) => return,
            (true, false) => Some(Disorder::new(None, group)),
            (false, false) => Some(Disorder::new(Some(assembly), group)),
        };
        let sites = self.disorder_sites();
        if !self.set_disorder(&sites, disorder, window, cx) {
            self.sync_disorder_fields(window, cx);
        }
    }

    /// The panel's "Remove disorder": the selected sites leave their
    /// assemblies.
    pub(crate) fn clear_disorder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sites = self.disorder_sites();
        self.set_disorder(&sites, None, window, cx);
    }

    /// Puts `sites` in `disorder`, or out of disorder for `None`, as one
    /// undo step naming only the sites that change (design §7.2). `false`
    /// when it was refused, which a notification explains.
    pub fn set_disorder(
        &mut self,
        sites: &[SiteId],
        disorder: Option<Disorder>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let structure = self.document.structure();
        let changes: Vec<SiteDisorder> = sites
            .iter()
            .filter(|&&site| {
                structure
                    .site(site)
                    .is_some_and(|site| site.disorder != disorder)
            })
            .map(|&site| SiteDisorder::new(site, disorder.clone()))
            .collect();
        if changes.is_empty() {
            return true;
        }
        self.edit(Edit::SetDisorder { sites: changes }, window, cx)
            .is_ok()
    }

    /// The note under the disorder fields: what they need, which sites
    /// they change, and that negative groups are not interpreted.
    pub(crate) fn disorder_notes(&self, cx: &App) -> Vec<SharedString> {
        let mut notes = Vec::new();
        let (assembly, group) = self.disorder.typed(cx);
        if !assembly.is_empty() && group.is_empty() {
            notes.push(tr!("panel.site.disorder_needs_group"));
        }
        let structure = self.document.structure();
        let sites = self.disorder_sites();
        if sites.len() > 1 {
            let labels: Vec<String> = sites
                .iter()
                .filter_map(|&site| structure.site(site))
                .map(|site| site.label().to_string())
                .collect();
            notes.push(if labels.len() <= 4 {
                tr!(
                    "panel.site.disorder_applies",
                    count = labels.len(),
                    labels = labels.join(&tr!("dialog.list_separator"))
                )
            } else {
                tr!(
                    "panel.site.disorder_applies_many",
                    count = labels.len(),
                    labels = labels[..3].join(&tr!("dialog.list_separator")),
                    rest = labels.len() - 3
                )
            });
        }
        let negative = sites
            .iter()
            .filter_map(|&site| structure.site(site)?.disorder.as_ref())
            .any(is_negative);
        if negative {
            notes.push(tr!("panel.site.disorder_negative"));
        }
        notes
    }

    /// Whether any selected site is disordered, so "Remove disorder" has
    /// something to do.
    pub(crate) fn selection_has_disorder(&self) -> bool {
        let structure = self.document.structure();
        self.disorder_sites()
            .iter()
            .any(|&site| structure.site(site).is_some_and(|s| s.disorder.is_some()))
    }

    // MARK: Groups shown

    /// Which group of each disorder assembly the viewport shows.
    pub fn shown_groups(&self) -> &ShownGroups {
        &self.shown_groups
    }

    /// Chooses the groups the viewport shows. View state: no undo step.
    pub fn set_shown_groups(&mut self, groups: ShownGroups, cx: &mut Context<Self>) {
        if self.shown_groups != groups {
            self.shown_groups = groups;
            self.refresh_scene(cx);
            cx.notify();
        }
    }

    /// Shows only `group` of `assembly`, or all its groups for `None`.
    pub fn show_disorder_group(
        &mut self,
        assembly: Option<&str>,
        group: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let mut groups = self.shown_groups.clone();
        groups.set(assembly, group);
        self.set_shown_groups(groups, cx);
    }

    /// Whether the structure has disordered sites to choose a group of; the
    /// toolbar offers its popover only then.
    pub(crate) fn has_disorder_groups(&self) -> bool {
        self.document
            .structure()
            .sites
            .iter()
            .filter_map(|site| site.disorder.as_ref())
            .any(|disorder| disorder.group != "0")
    }

    /// Opens or closes the popover that chooses the groups shown.
    pub fn set_disorder_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let open = open && self.has_disorder_groups();
        if self.disorder_open != open {
            self.disorder_open = open;
            cx.notify();
        }
    }

    /// The command palette's "Disorder…": the popover, or why there is
    /// none.
    pub(crate) fn edit_disorder_groups(
        &mut self,
        _: &EditDisorderGroups,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_disorder_groups() {
            self.set_disorder_open(true, cx);
        } else {
            window.push_notification(Notification::info(tr!("disorder.none")), cx);
        }
    }
}

/// The popover's content: for each disorder assembly, segments for all its
/// groups or one of them. Changes take effect at once.
pub(crate) struct DisorderPanel {
    workspace: WeakEntity<StructureWorkspace>,
}

impl DisorderPanel {
    pub(crate) fn new(workspace: WeakEntity<StructureWorkspace>) -> Self {
        Self { workspace }
    }
}

impl Render for DisorderPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let Some(workspace) = self.workspace.upgrade() else {
            return div().into_any_element();
        };
        let (assemblies, shown) = {
            let workspace = workspace.read(cx);
            let structure = workspace.document.structure();
            let assemblies = disorder_assemblies(structure);
            let shown: Vec<Option<String>> = assemblies
                .iter()
                .map(|a| {
                    let assembly = a.assembly.as_deref();
                    let group = workspace.shown_groups.shown(structure, assembly);
                    group.map(str::to_string)
                })
                .collect();
            (assemblies, shown)
        };
        let label = |text: SharedString| div().text_xs().text_color(muted).child(text);
        let named = assemblies.len() > 1;

        let rows = assemblies.into_iter().zip(shown).map(|(assembly, shown)| {
            let name = assembly.assembly.clone();
            let key = name.clone().unwrap_or_default();
            let groups = assembly.groups;
            let segments = ButtonGroup::new(SharedString::from(format!("disorder-groups-{key}")))
                .outline()
                .small()
                .child(
                    Button::new(SharedString::from(format!("disorder-all-{key}")))
                        .label(tr!("disorder.all"))
                        .selected(shown.is_none()),
                )
                .children(groups.iter().map(|group| {
                    Button::new(SharedString::from(format!("disorder-group-{key}-{group}")))
                        .label(group.clone())
                        .selected(shown.as_deref() == Some(group.as_str()))
                }))
                .on_click(cx.listener(move |this, clicks: &Vec<usize>, _, cx| {
                    let Some(&clicked) = clicks.first() else {
                        return;
                    };
                    let group = clicked.checked_sub(1).and_then(|i| groups.get(i)).cloned();
                    let assembly = name.clone();
                    this.workspace
                        .update(cx, |workspace, cx| {
                            workspace.show_disorder_group(assembly.as_deref(), group.as_deref(), cx)
                        })
                        .ok();
                }));
            v_flex()
                .gap_1()
                .when(named, |this| {
                    this.child(label(match &assembly.assembly {
                        Some(name) => tr!("disorder.assembly", name = name.clone()),
                        None => tr!("disorder.unnamed"),
                    }))
                })
                .child(segments)
        });

        v_flex()
            .id("disorder-panel")
            .w_64()
            .gap_3()
            .text_sm()
            .child(div().font_medium().child(tr!("disorder.title")))
            .children(rows)
            .child(label(tr!("disorder.hint")))
            .into_any_element()
    }
}
