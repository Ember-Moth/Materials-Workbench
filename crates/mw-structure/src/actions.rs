use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::{Action, App, KeyBinding, SharedString};

gpui_kit::actions!(
    structure,
    [
        TogglePanel,
        ResetView,
        ToggleMeasure,
        ClearSelection,
        DeleteSelection,
        AddMixedOccupant,
        Undo,
        Redo,
        ConvertToP1,
        MakeSupercell,
        ExportStructure,
        ShowBallAndStick,
        ShowSpaceFilling,
        ShowPolyhedra,
        ShowPropertiesPanel,
        ShowSitesPanel,
        ShowMeasurementsPanel,
        ShowDiagnosticsPanel,
    ]
);

/// Key context of the structure workspace; bindings scoped to it only work
/// while focus is inside the workspace.
pub const KEY_CONTEXT: &str = "StructureWorkspace";

/// Registers the workspace's key bindings (`docs/design/ui-shell.md` §8).
pub fn bind_keys(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("cmd-alt-b", TogglePanel, context),
        KeyBinding::new("cmd-0", ResetView, context),
        KeyBinding::new("cmd-e", ExportStructure, context),
        // Text fields undo their own typing in their deeper context.
        KeyBinding::new("cmd-z", Undo, context),
        KeyBinding::new("cmd-shift-z", Redo, context),
        // Text fields bind these keys in their own, deeper context, so typing
        // still edits text.
        KeyBinding::new("m", ToggleMeasure, context),
        KeyBinding::new("escape", ClearSelection, context),
        KeyBinding::new("backspace", DeleteSelection, context),
        KeyBinding::new("delete", DeleteSelection, context),
    ]);
}

/// A command the window's command palette offers for this workspace.
pub struct PaletteCommand {
    pub label: SharedString,
    /// Extra search terms: English names and pinyin, so either language
    /// finds the command.
    pub keywords: Vec<SharedString>,
    pub icon: Icon,
    pub action: Box<dyn Action>,
}

/// The workspace's commands, in palette order.
pub fn palette_commands() -> Vec<PaletteCommand> {
    let command =
        |label: SharedString, keywords: &[&str], icon: Lucide, action: Box<dyn Action>| {
            PaletteCommand {
                label,
                keywords: keywords
                    .iter()
                    .map(|k| SharedString::from(k.to_string()))
                    .collect(),
                icon: Icon::new(icon),
                action,
            }
        };
    vec![
        command(
            tr!("command.undo"),
            &["undo", "chexiao"],
            Lucide::Undo2,
            Box::new(Undo),
        ),
        command(
            tr!("command.redo"),
            &["redo", "chongzuo"],
            Lucide::Redo2,
            Box::new(Redo),
        ),
        command(
            tr!("command.make_supercell"),
            &["supercell", "chaobao", "chao bao"],
            Lucide::Grid3x3,
            Box::new(MakeSupercell),
        ),
        command(
            tr!("command.convert_to_p1"),
            &["convert to p1", "expand", "zhuanwei", "p1"],
            Lucide::Box,
            Box::new(ConvertToP1),
        ),
        command(
            tr!("command.export"),
            &["export", "cif", "poscar", "daochu"],
            Lucide::Download,
            Box::new(ExportStructure),
        ),
        command(
            tr!("command.add_mixed_occupant"),
            &[
                "mixed occupancy",
                "add occupant",
                "hunhe zhanwei",
                "tianjia zhanwei",
            ],
            Lucide::Plus,
            Box::new(AddMixedOccupant),
        ),
        command(
            tr!("command.delete_selection"),
            &["delete", "remove", "shanchu"],
            Lucide::Trash,
            Box::new(DeleteSelection),
        ),
        command(
            tr!("command.toggle_measure"),
            &["measure", "distance", "celiang"],
            Lucide::Ruler,
            Box::new(ToggleMeasure),
        ),
        command(
            tr!("command.clear_selection"),
            &[
                "clear selection",
                "deselect",
                "qingchu xuanze",
                "quxiao xuanze",
            ],
            Lucide::X,
            Box::new(ClearSelection),
        ),
        command(
            tr!("command.ball_and_stick"),
            &["ball and stick", "qiugun"],
            Lucide::Atom,
            Box::new(ShowBallAndStick),
        ),
        command(
            tr!("command.space_filling"),
            &["space filling", "kongjiantianchong"],
            Lucide::Circle,
            Box::new(ShowSpaceFilling),
        ),
        command(
            tr!("command.polyhedra"),
            &["polyhedra", "duomianti"],
            Lucide::Box,
            Box::new(ShowPolyhedra),
        ),
        command(
            tr!("command.reset_view"),
            &["reset view", "zhongzhi", "chongzhi"],
            Lucide::Maximize,
            Box::new(ResetView),
        ),
        command(
            tr!("command.toggle_panel"),
            &["panel", "inspector", "mianban"],
            Lucide::PanelRight,
            Box::new(TogglePanel),
        ),
    ]
}
