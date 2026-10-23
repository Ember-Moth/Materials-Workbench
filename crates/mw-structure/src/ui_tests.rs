//! UI integration tests (`docs/development.md` §4): the workspace in a
//! headless window, driven like a user drives it, by focusing fields,
//! typing and pressing keys. Each test checks what the user sees in the
//! fields and what changed in the document.
//!
//! GPUI Kit's interaction helpers (`gpui_kit::test`) need Kit's own
//! `test-support` feature, which conflicts with this workspace's
//! dependencies (see `Cargo.toml`). GPUI's test contexts are used directly:
//! keystrokes and typed text go through the real key bindings and inputs.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::InputState;
use gpui_kit::{
    AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds,
    WindowOptions, px, size,
};
use mw_crystal::sample::SampleKey;
use mw_crystal::{AtomRef, CellParam, SiteId};

use crate::document::StructureDocument;
use crate::selection::Selection;
use crate::workspace::StructureWorkspace;

/// Opens a sample in a workspace, the way the shell does, and renders it.
fn open(
    cx: &mut TestAppContext,
    key: SampleKey,
) -> (Entity<StructureWorkspace>, VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::bind_keys(cx);
    });
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            Default::default(),
            size(px(1280.), px(800.)),
        ))),
        ..Default::default()
    };
    let (window, workspace) = cx.update(|cx| {
        gpui_kit::open_window(options, cx, |window, cx| {
            let document = StructureDocument::from_sample("Samples", "Test", key);
            cx.new(|cx| StructureWorkspace::new(document, window, cx))
        })
        .expect("open a test window")
    });
    let mut cx = VisualTestContext::from_window(window, cx);
    focus_workspace(&workspace, &mut cx);
    (workspace, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
}

/// Moves keyboard focus to the workspace, as clicking the 3D view does.
fn focus_workspace(workspace: &Entity<StructureWorkspace>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let handle = workspace.read(cx).focus_handle.clone();
        handle.focus(window, cx);
    });
    draw(cx);
}

/// Focuses a field, replaces its text and presses Enter, as typing does.
fn type_into(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
    cx.update(|window, cx| input.update(cx, |state, cx| state.focus(window, cx)));
    draw(cx);
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input(text);
    cx.simulate_keystrokes("enter");
    draw(cx);
}

fn text(input: &Entity<InputState>, cx: &mut VisualTestContext) -> String {
    input.read_with(cx, |state, _| state.value().to_string())
}

fn cell_input(
    workspace: &Entity<StructureWorkspace>,
    param: CellParam,
    cx: &mut VisualTestContext,
) -> Entity<InputState> {
    workspace.read_with(cx, |workspace, _| {
        let (_, input) = workspace
            .cell_inputs
            .iter()
            .find(|(p, _)| *p == param)
            .expect("a field per parameter");
        input.clone()
    })
}

fn cell(
    workspace: &Entity<StructureWorkspace>,
    param: CellParam,
    cx: &mut VisualTestContext,
) -> f64 {
    workspace.read_with(cx, |workspace, _| {
        let params = workspace.document.structure().lattice.params();
        params.get(param).value()
    })
}

fn steps(workspace: &Entity<StructureWorkspace>, cx: &mut VisualTestContext) -> usize {
    workspace.read_with(cx, |workspace, _| {
        workspace.document.crystal.history().count()
    })
}

fn sites(workspace: &Entity<StructureWorkspace>, cx: &mut VisualTestContext) -> Vec<SiteId> {
    workspace.read_with(cx, |workspace, _| {
        workspace
            .document
            .structure()
            .sites
            .iter()
            .map(|s| s.id)
            .collect()
    })
}

fn select(
    workspace: &Entity<StructureWorkspace>,
    selection: Selection,
    cx: &mut VisualTestContext,
) {
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.set_selection(selection, window, cx)
        })
    });
    focus_workspace(workspace, cx);
}

/// How many atoms the 3D view rings as selected, and whether the image on
/// screen is the last frame drawn (so it shows those rings).
fn ringed(workspace: &Entity<StructureWorkspace>, cx: &mut VisualTestContext) -> (usize, bool) {
    workspace.read_with(cx, |workspace, cx| {
        let viewport = workspace.viewport.read(cx);
        (viewport.selection().len(), viewport.is_current())
    })
}

// An edit replaces the scene and then restores the selection rings, which
// submits two frames back to back. They finish on background threads in
// either order; the rings went missing when the older frame finished last
// and replaced the newer one. Several seeds let the test scheduler run them
// in both orders.
#[gpui_kit::test(iterations = 20)]
fn the_selection_stays_ringed_through_edits(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::StrontiumTitanate);
    // SrTiO₃ opens on Sr1, shown as the 8 corners of the cell.
    let selected = workspace.read_with(&cx, |workspace, _| workspace.selected_site());
    assert_eq!(selected, Some(SiteId(1)));
    assert_eq!(ringed(&workspace, &mut cx), (8, true));

    let a = cell_input(&workspace, CellParam::A, &mut cx);
    type_into(&mut cx, &a, "4.2");
    assert_eq!(cell(&workspace, CellParam::A, &mut cx), 4.2);
    let selected = workspace.read_with(&cx, |workspace, _| workspace.selected_site());
    assert_eq!(selected, Some(SiteId(1)));
    assert_eq!(ringed(&workspace, &mut cx), (8, true));

    focus_workspace(&workspace, &mut cx);
    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(ringed(&workspace, &mut cx), (8, true));
}

#[gpui_kit::test]
fn typed_cell_parameters_follow_the_symmetry_and_undo(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::Rutile);
    let a = cell_input(&workspace, CellParam::A, &mut cx);
    let b = cell_input(&workspace, CellParam::B, &mut cx);

    type_into(&mut cx, &a, "4.7");
    assert_eq!(cell(&workspace, CellParam::A, &mut cx), 4.7);
    // b = a in a tetragonal cell: the linked field shows it too.
    assert_eq!(cell(&workspace, CellParam::B, &mut cx), 4.7);
    assert_eq!(text(&b, &mut cx), "4.700");
    // Leaving the field with its value unchanged adds no step.
    focus_workspace(&workspace, &mut cx);
    assert_eq!(steps(&workspace, &mut cx), 1);

    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(cell(&workspace, CellParam::B, &mut cx), 4.594);
    assert_eq!(text(&a, &mut cx), "4.594");
    cx.simulate_keystrokes("cmd-shift-z");
    draw(&mut cx);
    assert_eq!(cell(&workspace, CellParam::A, &mut cx), 4.7);
}

#[gpui_kit::test]
fn refused_values_leave_the_structure_and_the_field_as_they_were(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::Rutile);
    // The workspace opens on O1, the first site with a free coordinate.
    let occupancy = workspace.read_with(&cx, |workspace, _| {
        assert_eq!(workspace.selected_site(), Some(SiteId(2)));
        workspace.occupants[0].occupancy.clone()
    });
    for typed in ["1.5", "0", "full"] {
        type_into(&mut cx, &occupancy, typed);
        assert_eq!(text(&occupancy, &mut cx), "1.000", "{typed}");
    }
    let c = cell_input(&workspace, CellParam::C, &mut cx);
    type_into(&mut cx, &c, "-1");
    assert_eq!(text(&c, &mut cx), "2.959");
    assert_eq!(steps(&workspace, &mut cx), 0);
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
}

#[gpui_kit::test]
fn mixed_occupancy_from_the_occupant_fields(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::Rutile);
    // Add a second occupant to O1 through the document, then edit both
    // from their own fields.
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            assert!(workspace.add_occupant(SiteId(2), "F", 0.25, window, cx));
        })
    });
    draw(&mut cx);
    let (o, f) = workspace.read_with(&cx, |workspace, _| {
        let labels: Vec<&str> = workspace
            .occupants
            .iter()
            .map(|e| e.label.as_ref())
            .collect();
        assert_eq!(labels, ["O1", "F1"]);
        (
            workspace.occupants[0].occupancy.clone(),
            workspace.occupants[1].occupancy.clone(),
        )
    });
    type_into(&mut cx, &o, "0.75");
    let occupancies = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |workspace, _| {
            let site = workspace.document.structure().site(SiteId(2)).unwrap();
            site.occupants
                .iter()
                .map(|o| o.occupancy.value())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(occupancies(&mut cx), [0.75, 0.25]);
    // The fields were kept, not made again: F1's still edits F1.
    type_into(&mut cx, &f, "0.2");
    assert_eq!(occupancies(&mut cx), [0.75, 0.2]);
    focus_workspace(&workspace, &mut cx);
    cx.simulate_keystrokes("cmd-z cmd-z cmd-z");
    draw(&mut cx);
    assert_eq!(occupancies(&mut cx), [1.0]);
    let labels = workspace.read_with(&cx, |workspace, _| workspace.occupants.len());
    assert_eq!(labels, 1);
}

#[gpui_kit::test]
fn the_element_picker_replaces_the_element(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::Rutile);
    let picker = workspace.read_with(&cx, |workspace, _| workspace.occupants[0].element.clone());
    cx.update(|window, cx| picker.update(cx, |state, cx| state.focus(window, cx)));
    draw(&mut cx);
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    cx.simulate_input("Se");
    draw(&mut cx);
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    let element = workspace.read_with(&cx, |workspace, _| {
        let site = workspace.document.structure().site(SiteId(2)).unwrap();
        (site.element().to_string(), site.label().to_string())
    });
    assert_eq!(element, ("Se".to_string(), "O1".to_string()));
}

#[gpui_kit::test]
fn delete_removes_whole_sites_and_undo_brings_them_back(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::StrontiumTitanate);
    select(&workspace, Selection::site(SiteId(3)), &mut cx);
    cx.simulate_keystrokes("delete");
    draw(&mut cx);
    assert_eq!(sites(&workspace, &mut cx), [SiteId(1), SiteId(2)]);
    // No confirmation: deleting can be undone.
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    let selected = workspace.read_with(&cx, |workspace, _| workspace.selected_site());
    assert_eq!(selected, None);

    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(
        sites(&workspace, &mut cx),
        [SiteId(1), SiteId(2), SiteId(3)]
    );
}

#[gpui_kit::test]
fn deleting_one_equivalent_atom_asks_instead(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::StrontiumTitanate);
    // One of the three O atoms of the cell.
    select(
        &workspace,
        Selection::atom(AtomRef::new(SiteId(3), 1, [0; 3])),
        &mut cx,
    );
    cx.simulate_keystrokes("delete");
    draw(&mut cx);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    assert_eq!(sites(&workspace, &mut cx).len(), 3);

    cx.simulate_keystrokes("escape");
    draw(&mut cx);
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    assert_eq!(sites(&workspace, &mut cx).len(), 3);
    assert_eq!(steps(&workspace, &mut cx), 0);
}

#[gpui_kit::test]
fn coordinates_move_atoms_in_p1(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::RutileSupercell);
    let (site, x) = workspace.read_with(&cx, |workspace, _| {
        (
            workspace.selected_site().unwrap(),
            workspace.coordinate_inputs[0].clone(),
        )
    });
    type_into(&mut cx, &x, "0.1");
    let position = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |workspace, _| {
            workspace.document.structure().site(site).unwrap().position
        })
    };
    assert_eq!(position(&mut cx)[0], 0.1);
}

#[gpui_kit::test]
fn coordinates_under_symmetry_stay_for_now(cx: &mut TestAppContext) {
    // Moving sites under symmetry is v0.2: the field shows the coordinate
    // again and nothing changes.
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    let x = rutile.read_with(&cx, |workspace, _| workspace.coordinate_inputs[0].clone());
    type_into(&mut cx, &x, "0.31");
    assert_eq!(text(&x, &mut cx), "0.3048");
    assert_eq!(steps(&rutile, &mut cx), 0);
}
