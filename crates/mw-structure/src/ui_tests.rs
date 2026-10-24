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
use std::time::Duration;

use gpui_kit::{
    AppContext as _, Bounds, Entity, Focusable as _, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PlatformInput, TestAppContext, VisualTestContext, WindowBounds,
    WindowOptions, point, px, size,
};
use mw_crystal::sample::SampleKey;
use mw_crystal::{AtomRef, CellParam, Command, EditMode, Measurement, SiteId, Target};
use mw_viewport::{CameraPose, Pick, Viewport};

use crate::selection::Selection;
use crate::workspace::{StructureWorkspace, StructureWorkspaceEvent};

/// Opens a sample in a workspace, the way the shell does, and renders it.
fn open(
    cx: &mut TestAppContext,
    key: SampleKey,
) -> (Entity<StructureWorkspace>, VisualTestContext) {
    open_document(cx, crate::document::tests::sample_document(key))
}

/// Opens a structure that is not a sample.
fn open_structure(
    cx: &mut TestAppContext,
    structure: mw_crystal::Structure,
) -> (Entity<StructureWorkspace>, VisualTestContext) {
    open_document(cx, crate::document::tests::document_of(structure))
}

fn open_document(
    cx: &mut TestAppContext,
    document: crate::document::StructureDocument,
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

/// Focuses a field and replaces its text without pressing Enter, which in a
/// dialog confirms it.
fn type_text(cx: &mut VisualTestContext, input: &Entity<InputState>, text: &str) {
    cx.update(|window, cx| input.update(cx, |state, cx| state.focus(window, cx)));
    draw(cx);
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input(text);
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

fn viewport(
    workspace: &Entity<StructureWorkspace>,
    cx: &mut VisualTestContext,
) -> Entity<Viewport> {
    workspace.read_with(cx, |workspace, _| workspace.viewport.clone())
}

/// Frames the 3D view has rendered so far.
fn frames(workspace: &Entity<StructureWorkspace>, cx: &mut VisualTestContext) -> u64 {
    workspace.read_with(cx, |workspace, cx| {
        workspace.viewport.read(cx).frames_submitted()
    })
}

// An edit replaces the scene and then restores the selection rings, which
// used to submit two frames back to back. They finish on background threads in
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

/// The labels of a site's occupants in the document.
fn labels(
    workspace: &Entity<StructureWorkspace>,
    site: SiteId,
    cx: &mut VisualTestContext,
) -> Vec<String> {
    workspace.read_with(cx, |workspace, _| {
        let site = workspace.document.structure().site(site).unwrap();
        site.occupants.iter().map(|o| o.label.clone()).collect()
    })
}

fn notifications(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.notifications(cx).len())
}

#[gpui_kit::test]
fn labels_are_renamed_from_their_field_and_undo(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::Rutile);
    // The workspace opens on O1.
    let field = workspace.read_with(&cx, |workspace, _| {
        workspace.occupants[0].label_input.clone()
    });
    // Spaces around the label are dropped.
    type_into(&mut cx, &field, " O2 ");
    assert_eq!(labels(&workspace, SiteId(2), &mut cx), ["O2"]);
    assert_eq!(text(&field, &mut cx), "O2");
    assert_eq!(steps(&workspace, &mut cx), 1);
    // The field was kept, with its focus, and now edits O2.
    let (kept, focused) = cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            let editor = &workspace.occupants[0];
            (
                editor.label_input == field && editor.label == "O2",
                editor.label_input.focus_handle(cx).is_focused(window),
            )
        })
    });
    assert!(kept && focused);
    // Leaving the field with its label unchanged adds no step.
    focus_workspace(&workspace, &mut cx);
    assert_eq!(steps(&workspace, &mut cx), 1);
    let occupancy =
        workspace.read_with(&cx, |workspace, _| workspace.occupants[0].occupancy.clone());
    type_into(&mut cx, &occupancy, "0.5");
    assert_eq!(steps(&workspace, &mut cx), 2);

    focus_workspace(&workspace, &mut cx);
    cx.simulate_keystrokes("cmd-z cmd-z");
    draw(&mut cx);
    assert_eq!(labels(&workspace, SiteId(2), &mut cx), ["O1"]);
    assert_eq!(text(&field, &mut cx), "O1");
    cx.simulate_keystrokes("cmd-shift-z");
    draw(&mut cx);
    assert_eq!(labels(&workspace, SiteId(2), &mut cx), ["O2"]);
    assert_eq!(text(&field, &mut cx), "O2");
}

#[gpui_kit::test]
fn refused_labels_leave_the_field_as_it_was(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::Rutile);
    let field = workspace.read_with(&cx, |workspace, _| {
        workspace.occupants[0].label_input.clone()
    });
    // Taken by the other site, empty, with a space inside, and starting
    // with a character CIF reserves.
    for (typed, refused) in [("Ti1", 1), ("", 2), ("O 1", 3), ("_O1", 4)] {
        type_into(&mut cx, &field, typed);
        assert_eq!(text(&field, &mut cx), "O1", "{typed:?}");
        assert_eq!(notifications(&mut cx), refused, "{typed:?}");
    }
    assert_eq!(labels(&workspace, SiteId(2), &mut cx), ["O1"]);
    assert_eq!(steps(&workspace, &mut cx), 0);
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));

    // Two occupants of one site cannot share a label either.
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            assert!(workspace.add_occupant(SiteId(2), "F", 0.25, window, cx));
        })
    });
    draw(&mut cx);
    let f = workspace.read_with(&cx, |workspace, _| {
        workspace.occupants[1].label_input.clone()
    });
    type_into(&mut cx, &f, "O1");
    assert_eq!(labels(&workspace, SiteId(2), &mut cx), ["O1", "F1"]);
    assert_eq!(text(&f, &mut cx), "F1");
    assert_eq!(steps(&workspace, &mut cx), 1);
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
    // The notification names the site and its atoms, which are gone from
    // the view (design §7.6): O1, 3 atoms.
    let message = workspace.read_with(&cx, |workspace, _| {
        workspace.deletion_message(&[SiteId(3)]).to_string()
    });
    assert!(message.contains("O1") && message.contains('3'), "{message}");
    let before = notifications(&mut cx);
    cx.simulate_keystrokes("delete");
    draw(&mut cx);
    assert_eq!(sites(&workspace, &mut cx), [SiteId(1), SiteId(2)]);
    assert_eq!(notifications(&mut cx), before + 1);
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
fn a_cell_off_its_symmetry_is_aligned_in_one_step(cx: &mut TestAppContext) {
    // SrTiO3 as a file might give it: b and c to other decimals, α off 90°
    // (design §7.3).
    let mut structure = mw_crystal::sample::load(SampleKey::StrontiumTitanate).structure;
    let off = mw_crystal::CellParams::new(3.905, 3.9051, 3.9049, 90.01, 90.0, 90.0);
    structure.lattice.set_params(off);
    let (workspace, mut cx) = open_structure(cx, structure);
    let note = |workspace: &Entity<StructureWorkspace>, cx: &mut VisualTestContext| {
        workspace.read_with(cx, |workspace, _| {
            let structure = workspace.document.structure();
            crate::panel::cell_deviation_note(&structure.symmetry, structure.lattice.params())
        })
    };
    assert!(note(&workspace, &mut cx).is_some());

    cx.update(|window, cx| window.dispatch_action(Box::new(crate::AlignCell), cx));
    draw(&mut cx);
    let params = [CellParam::A, CellParam::B, CellParam::C, CellParam::Alpha]
        .map(|param| cell(&workspace, param, &mut cx));
    assert_eq!(params, [3.905, 3.905, 3.905, 90.0]);
    assert_eq!(steps(&workspace, &mut cx), 1);
    assert!(note(&workspace, &mut cx).is_none());
    // Aligned already: nothing to do, no step.
    cx.update(|window, cx| window.dispatch_action(Box::new(crate::AlignCell), cx));
    draw(&mut cx);
    assert_eq!(steps(&workspace, &mut cx), 1);

    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(cell(&workspace, CellParam::B, &mut cx), 3.9051);
    assert_eq!(cell(&workspace, CellParam::Alpha, &mut cx), 90.01);
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

fn position(
    workspace: &Entity<StructureWorkspace>,
    site: SiteId,
    cx: &mut VisualTestContext,
) -> [f64; 3] {
    workspace.read_with(cx, |workspace, _| {
        workspace.document.structure().site(site).unwrap().position
    })
}

fn multiplicity(
    workspace: &Entity<StructureWorkspace>,
    site: SiteId,
    cx: &mut VisualTestContext,
) -> usize {
    workspace.read_with(cx, |workspace, _| {
        workspace.document.derived().multiplicity(site).unwrap()
    })
}

#[gpui_kit::test]
fn typed_coordinates_keep_the_special_position(cx: &mut TestAppContext) {
    // Rutile opens on O1, on 4f: x, x, 0.
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    let o1 = SiteId(2);
    let [x, y, _] = rutile.read_with(&cx, |workspace, _| {
        assert_eq!(workspace.selected_site(), Some(o1));
        workspace.coordinate_inputs.clone()
    });
    // y follows x and can be typed: x changes with it (design §7.5).
    type_into(&mut cx, &y, "0.31");
    assert_eq!(position(&rutile, o1, &mut cx), [0.31, 0.31, 0.0]);
    assert_eq!(text(&x, &mut cx), "0.31");
    assert_eq!(steps(&rutile, &mut cx), 1);

    // x close to 0 puts O1 on 2a, onto Ti1: snapped, half the atoms, and
    // a warning that it sits on Ti1.
    let before = notifications(&mut cx);
    type_into(&mut cx, &x, "0.0001");
    assert_eq!(position(&rutile, o1, &mut cx), [0.0, 0.0, 0.0]);
    assert_eq!(multiplicity(&rutile, o1, &mut cx), 2);
    assert_eq!(notifications(&mut cx), before + 2);

    focus_workspace(&rutile, &mut cx);
    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(position(&rutile, o1, &mut cx), [0.31, 0.31, 0.0]);
    assert_eq!(text(&x, &mut cx), "0.31");
}

/// Points at the atoms of `site` in turn until one is not hidden behind
/// another, and returns where it is in the window, hovered.
fn hover_atom_of(
    workspace: &Entity<StructureWorkspace>,
    site: SiteId,
    cx: &mut VisualTestContext,
) -> (AtomRef, gpui_kit::Point<gpui_kit::Pixels>) {
    let candidates: Vec<(AtomRef, gpui_kit::Point<gpui_kit::Pixels>)> =
        workspace.read_with(cx, |workspace, cx| {
            let viewport = workspace.viewport.read(cx);
            let origin = viewport.bounds().expect("laid out").origin;
            workspace
                .document
                .derived()
                .atoms
                .iter()
                .filter(|atom| atom.atom.site == site)
                .filter_map(|atom| Some((atom.atom, origin + viewport.project(atom.position)?)))
                .collect()
        });
    for (atom, at) in candidates {
        cx.simulate_mouse_move(at, None, Modifiers::none());
        let hovered = workspace.read_with(cx, |workspace, cx| {
            workspace.picked_atom(workspace.viewport.read(cx).hovered())
        });
        if hovered == Some(atom) {
            return (atom, at);
        }
    }
    panic!("no atom of site {} is visible", site.0);
}

fn alt() -> Modifiers {
    Modifiers {
        alt: true,
        ..Modifiers::default()
    }
}

#[gpui_kit::test]
fn dragging_an_atom_moves_its_site_along_its_special_position(cx: &mut TestAppContext) {
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    let o1 = SiteId(2);
    cx.simulate_keystrokes("v");
    draw(&mut cx);
    assert!(rutile.read_with(&cx, |workspace, _| workspace.moving));
    let (atom, at) = hover_atom_of(&rutile, o1, &mut cx);

    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    let to = at + point(px(40.), px(25.));
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    draw(&mut cx);
    // A preview: the document is as it was, the prompt says where O1 goes.
    assert_eq!(steps(&rutile, &mut cx), 0);
    let prompt = rutile.read_with(&cx, |workspace, _| workspace.move_prompt().to_string());
    assert!(prompt.starts_with("O1 "), "{prompt}");
    assert!(!prompt.contains('→'), "{prompt}");

    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    draw(&mut cx);
    assert_eq!(steps(&rutile, &mut cx), 1);
    let [x, y, z] = position(&rutile, o1, &mut cx);
    assert!((x - y).abs() < 1e-12 && z == 0.0, "{:?}", [x, y, z]);
    assert!((x - 0.3048).abs() > 1e-3, "moved: {x}");
    assert_eq!(multiplicity(&rutile, o1, &mut cx), 4);
    // The dragged atom is selected, so the panel shows O1.
    let selected = rutile.read_with(&cx, |workspace, _| workspace.selection.targets());
    assert_eq!(selected, [Target::Atom(atom)]);

    focus_workspace(&rutile, &mut cx);
    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(position(&rutile, o1, &mut cx), [0.3048, 0.3048, 0.0]);
}

#[gpui_kit::test]
fn holding_alt_leaves_the_special_position(cx: &mut TestAppContext) {
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    let o1 = SiteId(2);
    // A distance to an O1 atom outside the cell: its line makes the scene
    // larger than the cell.
    let ti = AtomRef::new(SiteId(1), 0, [0; 3]);
    let far = AtomRef::new(o1, 0, [1, 1, 1]);
    cx.update(|window, cx| {
        rutile.update(cx, |workspace, cx| {
            workspace.execute(
                Command::AddMeasurement(Measurement::Distance([ti, far])),
                window,
                cx,
            )
        })
    })
    .expect("add the distance");
    cx.simulate_keystrokes("v");
    draw(&mut cx);
    let ti_on_screen = |cx: &mut VisualTestContext| {
        rutile.read_with(cx, |workspace, cx| {
            workspace.viewport.read(cx).project([0.0; 3]).unwrap()
        })
    };
    let before = ti_on_screen(&mut cx);
    let (_, at) = hover_atom_of(&rutile, o1, &mut cx);
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    let to = at + point(px(40.), px(25.));
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    // ⌥ alone, without moving, changes the preview.
    cx.simulate_modifiers_change(alt());
    draw(&mut cx);
    let prompt = rutile.read_with(&cx, |workspace, _| workspace.move_prompt().to_string());
    assert!(prompt.contains("→"), "{prompt}");
    // O1's member numbers change, so the distance's line goes; the view
    // stays where it was all the same.
    assert_eq!(ti_on_screen(&mut cx), before);

    let before = notifications(&mut cx);
    cx.simulate_mouse_up(to, MouseButton::Left, alt());
    draw(&mut cx);
    // The distance, then the move.
    assert_eq!(steps(&rutile, &mut cx), 2);
    assert!(multiplicity(&rutile, o1, &mut cx) > 4);
    // "O1 多重度 4 → …", and a warning if its atoms came close to each
    // other.
    assert!(notifications(&mut cx) > before);
}

#[gpui_kit::test]
fn escape_puts_a_dragged_atom_back(cx: &mut TestAppContext) {
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    let o1 = SiteId(2);
    cx.simulate_keystrokes("v");
    draw(&mut cx);
    let (_, at) = hover_atom_of(&rutile, o1, &mut cx);
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    let to = at + point(px(40.), px(25.));
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    draw(&mut cx);
    assert!(rutile.read_with(&cx, |workspace, _| workspace.drag.is_some()));
    cx.simulate_keystrokes("escape");
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    draw(&mut cx);
    assert!(rutile.read_with(&cx, |workspace, _| workspace.drag.is_none()));
    assert_eq!(steps(&rutile, &mut cx), 0);
    assert_eq!(position(&rutile, o1, &mut cx), [0.3048, 0.3048, 0.0]);
}

// Press, move and release arrive together, before any pick at the press
// position: the viewport waits for that pick to tell what was pressed, and
// the release then drops the atom.
#[gpui_kit::test]
fn a_drag_faster_than_the_pick_still_moves_the_atom(cx: &mut TestAppContext) {
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    let o1 = SiteId(2);
    cx.simulate_keystrokes("v");
    draw(&mut cx);
    let (_, at) = hover_atom_of(&rutile, o1, &mut cx);
    // Away from the atom, so the last hover pick says nothing about it.
    let away = rutile.read_with(&cx, |workspace, cx| {
        workspace.viewport.read(cx).bounds().unwrap().origin + point(px(8.), px(80.))
    });
    cx.simulate_mouse_move(away, None, Modifiers::none());
    let to = at + point(px(40.), px(25.));
    cx.update(|window, cx| {
        let left = MouseButton::Left;
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: at,
                pressed_button: None,
                modifiers: Modifiers::none(),
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: left,
                position: at,
                modifiers: Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: to,
                pressed_button: Some(left),
                modifiers: Modifiers::none(),
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: left,
                position: to,
                modifiers: Modifiers::none(),
                click_count: 1,
            }),
            cx,
        );
    });
    draw(&mut cx);
    assert_eq!(steps(&rutile, &mut cx), 1);
    assert_ne!(position(&rutile, o1, &mut cx), [0.3048, 0.3048, 0.0]);
}

#[gpui_kit::test]
fn outside_move_mode_dragging_turns_the_view(cx: &mut TestAppContext) {
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    let (_, at) = hover_atom_of(&rutile, SiteId(2), &mut cx);
    let pose = rutile.read_with(&cx, |workspace, cx| {
        workspace.viewport.read(cx).camera_pose().yaw
    });
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(
        at + point(px(40.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        at + point(px(40.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    draw(&mut cx);
    assert_eq!(steps(&rutile, &mut cx), 0);
    let turned = rutile.read_with(&cx, |workspace, cx| {
        workspace.viewport.read(cx).camera_pose().yaw
    });
    assert_ne!(turned, pose);
}

#[gpui_kit::test]
fn adding_sites(cx: &mut TestAppContext) {
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    // The dialog opens from the action, and Esc closes it.
    focus_workspace(&rutile, &mut cx);
    cx.update(|window, cx| window.dispatch_action(Box::new(crate::AddSite), cx));
    draw(&mut cx);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    cx.simulate_keystrokes("escape");
    draw(&mut cx);
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));

    // Typed near 2b, at 0, 0, 1/2: snapped there, and selected.
    let added = cx.update(|window, cx| {
        rutile.update(cx, |workspace, cx| {
            workspace.add_site_at([0.0003, 0.9998, 0.5001], "F", 1.0, window, cx)
        })
    });
    assert!(added);
    draw(&mut cx);
    assert_eq!(sites(&rutile, &mut cx), [SiteId(1), SiteId(2), SiteId(3)]);
    assert_eq!(position(&rutile, SiteId(3), &mut cx), [0.0, 1.0, 0.5]);
    assert_eq!(multiplicity(&rutile, SiteId(3), &mut cx), 2);
    let selected = rutile.read_with(&cx, |workspace, _| workspace.selected_site());
    assert_eq!(selected, Some(SiteId(3)));

    // On an atom of O1: nothing is added, and a dialog offers F as a mixed
    // occupant of O1 instead; Enter accepts.
    let offered = cx.update(|window, cx| {
        rutile.update(cx, |workspace, cx| {
            workspace.add_site_at([0.6952, 0.6952, 0.0], "F", 0.25, window, cx)
        })
    });
    assert!(offered);
    draw(&mut cx);
    assert_eq!(sites(&rutile, &mut cx).len(), 3);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    assert_eq!(labels(&rutile, SiteId(2), &mut cx), ["O1", "F2"]);
    assert_eq!(steps(&rutile, &mut cx), 2);
}

#[gpui_kit::test]
fn converting_to_p1_follows_the_selection_and_undoes_at_once(cx: &mut TestAppContext) {
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    let o = AtomRef::new(SiteId(2), 3, [0; 3]);
    select(&rutile, Selection::atom(o), &mut cx);
    cx.update(|window, cx| rutile.update(cx, |workspace, cx| workspace.expand_to_p1(window, cx)));
    draw(&mut cx);
    let (mode, selected, label) = rutile.read_with(&cx, |workspace, _| {
        let structure = workspace.document.structure();
        let selected = workspace.selected_site().unwrap();
        (
            structure.edit_mode(),
            workspace.selection.targets(),
            structure.site(selected).unwrap().label().to_string(),
        )
    });
    assert_eq!(mode, EditMode::P1);
    assert_eq!(sites(&rutile, &mut cx).len(), 6);
    // The selected O is the fourth member of O1: now site O1_4, member 0.
    assert_eq!(label, "O1_4");
    assert!(matches!(selected[..], [Target::Atom(atom)] if atom.member == 0));
    assert_eq!(steps(&rutile, &mut cx), 1);
    // The coordinate fields of a P1 site are free.
    assert_eq!(ringed(&rutile, &mut cx), (1, true));

    focus_workspace(&rutile, &mut cx);
    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(sites(&rutile, &mut cx), [SiteId(1), SiteId(2)]);
    let selected = rutile.read_with(&cx, |workspace, _| workspace.selection.targets());
    assert_eq!(selected, [Target::Atom(o)]);
}

#[gpui_kit::test]
fn the_delete_dialog_converts_to_p1_and_deletes_one_atom(cx: &mut TestAppContext) {
    let (srtio3, mut cx) = open(cx, SampleKey::StrontiumTitanate);
    let o = AtomRef::new(SiteId(3), 1, [0; 3]);
    select(&srtio3, Selection::atom(o), &mut cx);
    cx.simulate_keystrokes("delete");
    draw(&mut cx);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    // "Convert to P1 and delete this one", the dialog's default.
    cx.update(|window, cx| {
        window.close_dialog(cx);
        srtio3.update(cx, |workspace, cx| {
            let targets = workspace.selection.targets();
            workspace.delete_after_expanding(targets, window, cx)
        })
    });
    draw(&mut cx);
    let labels = srtio3.read_with(&cx, |workspace, _| {
        let structure = workspace.document.structure();
        assert_eq!(structure.edit_mode(), EditMode::P1);
        structure
            .sites
            .iter()
            .map(|s| s.label().to_string())
            .collect::<Vec<_>>()
    });
    assert_eq!(labels, ["Sr1", "Ti1", "O1_1", "O1_3"]);
    assert_eq!(steps(&srtio3, &mut cx), 1);
    focus_workspace(&srtio3, &mut cx);
    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(sites(&srtio3, &mut cx), [SiteId(1), SiteId(2), SiteId(3)]);
}

#[gpui_kit::test]
fn supercells_go_to_the_host(cx: &mut TestAppContext) {
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    let made = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _subscription = cx.update(|_, cx| {
        let made = made.clone();
        cx.subscribe(&rutile, move |_, event: &StructureWorkspaceEvent, _| {
            made.borrow_mut().push(event.clone())
        })
    });
    cx.update(|window, cx| {
        rutile.update(cx, |workspace, cx| {
            workspace.make_supercell_of([1, 1, 3], window, cx)
        })
    });
    draw(&mut cx);
    let made = made.borrow();
    let [StructureWorkspaceEvent::MadeSupercell(supercell)] = &made[..] else {
        panic!("one supercell");
    };
    // Named in the interface language, after the structure.
    assert!(
        supercell.name.starts_with("Test 1×1×3 "),
        "{}",
        supercell.name
    );
    assert_eq!(supercell.matrix, [[1, 0, 0], [0, 1, 0], [0, 0, 3]]);
    assert_eq!(supercell.structure.sites.len(), 18);
    assert!(supercell.source.symmetry.operations().len() > 1);
    // The structure itself does not change.
    assert_eq!(steps(&rutile, &mut cx), 0);
}

// The scene, the rings and the camera an edit changes go into one frame.
#[gpui_kit::test]
fn an_edit_renders_one_frame(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::StrontiumTitanate);
    let a = cell_input(&workspace, CellParam::A, &mut cx);
    let before = frames(&workspace, &mut cx);
    type_into(&mut cx, &a, "4.2");
    assert_eq!(cell(&workspace, CellParam::A, &mut cx), 4.2);
    assert_eq!(frames(&workspace, &mut cx), before + 1);
    assert_eq!(ringed(&workspace, &mut cx), (8, true));

    focus_workspace(&workspace, &mut cx);
    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(frames(&workspace, &mut cx), before + 2);
    assert_eq!(ringed(&workspace, &mut cx), (8, true));
}

// The hover ring is painted over the image: pointing at atoms picks them
// but renders nothing.
#[gpui_kit::test]
fn hovering_renders_no_frame(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::StrontiumTitanate);
    select(&workspace, Selection::default(), &mut cx);
    let before = frames(&workspace, &mut cx);
    let viewport = viewport(&workspace, &mut cx);
    let ti = workspace
        .read_with(&cx, |workspace, cx| {
            workspace.displayed_atom_position([0.5, 0.5, 0.5], cx)
        })
        .expect("Ti at the cell center");
    cx.simulate_mouse_move(ti, None, Modifiers::none());
    let hovered = viewport.read_with(&cx, |viewport, _| viewport.hovered());
    assert!(matches!(hovered, Some(Pick::Sphere(_))), "{hovered:?}");

    let corner = viewport
        .read_with(&cx, |viewport, _| viewport.bounds())
        .expect("laid out")
        .origin
        + point(px(8.), px(80.));
    cx.simulate_mouse_move(corner, None, Modifiers::none());
    assert_eq!(
        viewport.read_with(&cx, |viewport, _| viewport.hovered()),
        None
    );
    assert_eq!(frames(&workspace, &mut cx), before);
    assert!(viewport.read_with(&cx, |viewport, _| viewport.is_current()));
}

// Labels are placed with the camera of the image on screen, not with one
// whose frame is still being rendered, so they do not run ahead of the
// atoms while the view turns.
#[gpui_kit::test]
fn labels_follow_the_image_on_screen(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::StrontiumTitanate);
    let sr = AtomRef::new(SiteId(1), 0, [0; 3]);
    let ti = AtomRef::new(SiteId(2), 0, [0; 3]);
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            let distance = Measurement::Distance([sr, ti]);
            workspace.execute(Command::AddMeasurement(distance), window, cx)
        })
    })
    .expect("add the distance");
    draw(&mut cx);
    let labels = |cx: &mut VisualTestContext| {
        workspace.read_with(cx, |workspace, cx| {
            let labels = workspace.measurement_labels(cx);
            assert_eq!(labels.len(), 1);
            labels[0].0
        })
    };
    let viewport = viewport(&workspace, &mut cx);
    let before = labels(&mut cx);

    // Turn the view; the frame with the new camera is still in flight.
    cx.update(|_, cx| {
        viewport.update(cx, |viewport, cx| {
            let pose = viewport.camera_pose();
            let turned = CameraPose::new(pose.yaw + 0.5, pose.pitch, pose.half_height, pose.target);
            viewport.set_camera_pose(turned, cx)
        })
    });
    assert!(!viewport.read_with(&cx, |viewport, _| viewport.is_current()));
    assert_eq!(labels(&mut cx), before);

    cx.run_until_parked();
    assert!(viewport.read_with(&cx, |viewport, _| viewport.is_current()));
    assert_ne!(labels(&mut cx), before);
}

// Dragging renders at reduced resolution; once the camera rests, a full
// frame makes the image current again.
#[gpui_kit::test]
fn dragging_settles_into_a_full_frame(cx: &mut TestAppContext) {
    let (workspace, mut cx) = open(cx, SampleKey::StrontiumTitanate);
    let viewport = viewport(&workspace, &mut cx);
    let bounds = viewport
        .read_with(&cx, |viewport, _| viewport.bounds())
        .expect("laid out");
    let start = bounds.origin + point(px(40.), bounds.size.height / 2.);
    let before = frames(&workspace, &mut cx);
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    for step in 1..=3 {
        let to = start + point(px(20. * step as f32), px(0.));
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    }
    cx.simulate_mouse_up(
        start + point(px(60.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    // One frame per move, the last of them on screen but not yet full size.
    assert_eq!(frames(&workspace, &mut cx), before + 3);
    assert!(!viewport.read_with(&cx, |viewport, _| viewport.is_current()));

    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(frames(&workspace, &mut cx), before + 4);
    assert!(viewport.read_with(&cx, |viewport, _| viewport.is_current()));
}

/// Rock salt in F m -3 m, conventional cell: 8 atoms.
fn rock_salt() -> mw_crystal::Structure {
    use mw_crystal::{CellParams, Lattice, Measured, Occupant, Site, Species, Structure, Symmetry};
    let atom = |id: u32, label: &str, element: &str, position: [f64; 3]| {
        let occupant = Occupant::new(label, Species::element(element), Measured::exact(1.0));
        Site::new(SiteId(id), position, vec![occupant])
    };
    Structure::new(
        Lattice::new(CellParams::new(5.64, 5.64, 5.64, 90.0, 90.0, 90.0)),
        Symmetry::from_hm_symbol("F m -3 m").unwrap(),
        vec![
            atom(1, "Na1", "Na", [0.0, 0.0, 0.0]),
            atom(2, "Cl1", "Cl", [0.5, 0.5, 0.5]),
        ],
    )
}

fn cell_dialog(
    workspace: &Entity<StructureWorkspace>,
    cx: &mut VisualTestContext,
) -> Entity<crate::cell_dialog::CellDialog> {
    workspace.read_with(cx, |workspace, _| {
        workspace
            .cell_dialog
            .as_ref()
            .and_then(|dialog| dialog.upgrade())
            .expect("the cell dialog is open")
    })
}

#[gpui_kit::test]
fn transforming_the_cell(cx: &mut TestAppContext) {
    use crate::cell_dialog::Choice;
    use mw_crystal::CellPreset;

    let (salt, mut cx) = open_structure(cx, rock_salt());
    // A Cl atom on a face of the conventional cell is selected.
    let cl = AtomRef::new(SiteId(2), 3, [0; 3]);
    select(&salt, Selection::atom(cl), &mut cx);
    let cartesian = |workspace: &Entity<StructureWorkspace>, cx: &mut VisualTestContext| {
        workspace.read_with(cx, |workspace, _| {
            let [Target::Atom(atom)] = workspace.selection.targets()[..] else {
                panic!("one atom selected");
            };
            let x = workspace.document.derived().fractional(atom).unwrap();
            workspace.document.structure().lattice.to_cartesian(x)
        })
    };
    let before = cartesian(&salt, &mut cx);

    // The dialog opens from the action on the primitive cell, its P in the
    // fields and the atoms it would hold in the preview.
    cx.update(|window, cx| window.dispatch_action(Box::new(crate::TransformCell), cx));
    draw(&mut cx);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    let dialog = cell_dialog(&salt, &mut cx);
    let (choice, atoms, p01, p00) = dialog.read_with(&cx, |dialog, cx| {
        (
            dialog.chosen_value(cx),
            dialog.previewed_atoms(),
            dialog.matrix[0][1].read(cx).value().to_string(),
            dialog.matrix[0][0].read(cx).value().to_string(),
        )
    });
    assert_eq!(choice, Some(Choice::Preset(CellPreset::Primitive)));
    assert_eq!(atoms, Some(2));
    assert_eq!((p00.as_str(), p01.as_str()), ("0", "1/2"));

    // Typing a value of P makes the choice custom. One whose a′ is no
    // translation of the crystal is refused in the preview, and Enter, which
    // confirms the dialog, changes nothing and keeps it open.
    let field = dialog.read_with(&cx, |dialog, _| dialog.matrix[0][0].clone());
    type_text(&mut cx, &field, "1/4");
    let (choice, refusal) = dialog.read_with(&cx, |dialog, cx| {
        (dialog.chosen_value(cx), dialog.refusal())
    });
    assert_eq!(choice, Some(Choice::Custom));
    assert!(refusal.is_some_and(|reason| reason.contains("a′")));
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    assert_eq!(steps(&salt, &mut cx), 0);

    // Choosing the primitive cell again fills its P back; Enter makes one
    // step, and the selected atom is the same atom of the new cell.
    cx.update(|window, cx| {
        dialog.update(cx, |dialog, cx| {
            dialog.choose(Choice::Preset(CellPreset::Primitive), window, cx)
        })
    });
    let p00 = dialog.read_with(&cx, |dialog, cx| {
        dialog.matrix[0][0].read(cx).value().to_string()
    });
    assert_eq!(p00, "0");
    type_text(&mut cx, &field, "0");
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    assert_eq!(steps(&salt, &mut cx), 1);
    let (operations, atoms) = salt.read_with(&cx, |workspace, _| {
        let document = &workspace.document;
        (
            document.structure().symmetry.operations().len(),
            document.derived().atoms_in_cell(),
        )
    });
    assert_eq!((operations, atoms), (48, 2));
    let after = cartesian(&salt, &mut cx);
    for (a, b) in after.iter().zip(before) {
        assert!((a - b).abs() < 1e-9, "{after:?} {before:?}");
    }
    assert_eq!(cell(&salt, CellParam::Alpha, &mut cx), 60.0);
    // The cell field shows the new length.
    let a = cell_input(&salt, CellParam::A, &mut cx);
    assert_eq!(text(&a, &mut cx), "3.988");

    // One undo puts the conventional cell and the selection back.
    focus_workspace(&salt, &mut cx);
    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(cell(&salt, CellParam::Alpha, &mut cx), 90.0);
    let selected = salt.read_with(&cx, |workspace, _| workspace.selection.targets());
    assert_eq!(selected, [Target::Atom(cl)]);
}

#[gpui_kit::test]
fn the_cell_dialog_offers_what_the_structure_has(cx: &mut TestAppContext) {
    use crate::cell_dialog::Choice;
    use mw_crystal::CellPreset;

    // Rutile is primitive and in its standard setting: the dialog starts on
    // the Niggli cell, the one change that does something, and Esc closes
    // it.
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    focus_workspace(&rutile, &mut cx);
    cx.update(|window, cx| window.dispatch_action(Box::new(crate::TransformCell), cx));
    draw(&mut cx);
    let dialog = cell_dialog(&rutile, &mut cx);
    let choice = dialog.read_with(&cx, |dialog, cx| dialog.chosen_value(cx));
    assert_eq!(choice, Some(Choice::Preset(CellPreset::Niggli)));
    // The custom choice starts from the fields as they are; the identity
    // changes nothing.
    cx.update(|window, cx| {
        dialog.update(cx, |dialog, cx| dialog.choose(Choice::Custom, window, cx))
    });
    for (i, row) in [["1", "0", "0"], ["0", "1", "0"], ["0", "0", "1"]]
        .iter()
        .enumerate()
    {
        for (j, value) in row.iter().enumerate() {
            let field = dialog.read_with(&cx, |dialog, _| dialog.matrix[i][j].clone());
            type_text(&mut cx, &field, value);
        }
    }
    let (atoms, refusal) = dialog.read_with(&cx, |dialog, _| {
        (dialog.previewed_atoms(), dialog.refusal())
    });
    assert_eq!((atoms, refusal), (None, None));
    // Enter with nothing to change closes the dialog and adds no step.
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    assert_eq!(steps(&rutile, &mut cx), 0);
}

fn symmetrize_dialog(
    workspace: &Entity<StructureWorkspace>,
    cx: &mut VisualTestContext,
) -> Entity<crate::symmetrize_dialog::SymmetrizeDialog> {
    workspace.read_with(cx, |workspace, _| {
        workspace
            .symmetrize_dialog
            .as_ref()
            .and_then(|dialog| dialog.upgrade())
            .expect("the symmetrize dialog is open")
    })
}

/// Milestone M9: the P1 supercell gets rutile's symmetry back, previewed
/// in the background, in one undo step; the selected atom stays selected.
#[gpui_kit::test]
fn symmetrizing_a_p1_structure(cx: &mut TestAppContext) {
    let (supercell, mut cx) = open(cx, SampleKey::RutileSupercell);
    let o = supercell.read_with(&cx, |workspace, _| {
        let structure = workspace.document.structure();
        let site = structure.sites.iter().find(|s| s.element() == "O").unwrap();
        AtomRef::new(site.id, 0, [0; 3])
    });
    select(&supercell, Selection::atom(o), &mut cx);
    let cartesian = |workspace: &Entity<StructureWorkspace>, cx: &mut VisualTestContext| {
        workspace.read_with(cx, |workspace, _| {
            let [Target::Atom(atom)] = workspace.selection.targets()[..] else {
                panic!("one atom selected");
            };
            let x = workspace.document.derived().fractional(atom).unwrap();
            workspace.document.structure().lattice.to_cartesian(x)
        })
    };
    let before = cartesian(&supercell, &mut cx);

    cx.update(|window, cx| window.dispatch_action(Box::new(crate::Symmetrize), cx));
    draw(&mut cx);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    let dialog = symmetrize_dialog(&supercell, &mut cx);
    // Kept, the supercell holds P4₂/mnm with its translations as centering.
    let found = |dialog: &Entity<crate::symmetrize_dialog::SymmetrizeDialog>,
                 cx: &mut VisualTestContext| {
        dialog.read_with(cx, |dialog, _| {
            dialog.found().map(|found| {
                (
                    found.detected_number,
                    found.structure.symmetry.operations().len(),
                    found.structure.sites.len(),
                    found.cell_changed,
                )
            })
        })
    };
    assert_eq!(found(&dialog, &mut cx), Some((136, 128, 2, false)));
    // Standardized, it is the rutile cell.
    dialog.update(&mut cx, |dialog, cx| dialog.set_standardize(true, cx));
    draw(&mut cx);
    assert_eq!(found(&dialog, &mut cx), Some((136, 16, 2, true)));

    // A tolerance that is no number is refused in the preview, and Enter,
    // which confirms the dialog, changes nothing and keeps it open.
    let tolerance = dialog.read_with(&cx, |dialog, _| dialog.tolerance.clone());
    type_text(&mut cx, &tolerance, "a lot");
    assert!(
        dialog
            .read_with(&cx, |dialog, _| dialog.refusal())
            .is_some()
    );
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    assert_eq!(steps(&supercell, &mut cx), 0);

    type_text(&mut cx, &tolerance, "0.01");
    assert_eq!(found(&dialog, &mut cx), Some((136, 16, 2, true)));
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    assert_eq!(steps(&supercell, &mut cx), 1);
    let (mode, operations, atoms, labels) = supercell.read_with(&cx, |workspace, _| {
        let document = &workspace.document;
        let structure = document.structure();
        (
            structure.edit_mode(),
            structure.symmetry.operations().len(),
            document.derived().atoms_in_cell(),
            structure
                .sites
                .iter()
                .map(|s| s.label().to_string())
                .collect::<Vec<_>>(),
        )
    });
    assert_eq!((mode, operations, atoms), (EditMode::Symmetric, 16, 6));
    assert_eq!(labels, ["Ti1", "O1"]);
    // The selected oxygen is the same atom of the new cell.
    let after = cartesian(&supercell, &mut cx);
    for (a, b) in after.iter().zip(before) {
        assert!((a - b).abs() < 1e-9, "{after:?} {before:?}");
    }

    // One undo puts the supercell and the selection back.
    focus_workspace(&supercell, &mut cx);
    cx.simulate_keystrokes("cmd-z");
    draw(&mut cx);
    assert_eq!(sites(&supercell, &mut cx).len(), 48);
    let selected = supercell.read_with(&cx, |workspace, _| workspace.selection.targets());
    assert_eq!(selected, [Target::Atom(o)]);
}

/// A structure whose atoms have no more symmetry than its operations: the
/// preview says so, and confirming changes nothing.
#[gpui_kit::test]
fn nothing_to_symmetrize_says_why(cx: &mut TestAppContext) {
    let (rutile, mut cx) = open(cx, SampleKey::Rutile);
    cx.update(|window, cx| window.dispatch_action(Box::new(crate::Symmetrize), cx));
    draw(&mut cx);
    let dialog = symmetrize_dialog(&rutile, &mut cx);
    let refusal = dialog.read_with(&cx, |dialog, _| dialog.refusal());
    assert_eq!(
        refusal.as_deref(),
        Some(crate::editing::symmetrize_error_text(
            &mw_crystal::SymmetrizeError::NoHigherSymmetry,
            &rutile.read_with(&cx, |workspace, _| workspace.document.structure().clone()),
        ))
        .as_deref()
    );
    let shown = notifications(&mut cx);
    cx.simulate_keystrokes("enter");
    draw(&mut cx);
    cx.update(|window, cx| assert!(window.has_active_dialog(cx)));
    assert_eq!(notifications(&mut cx), shown + 1);
    assert_eq!(steps(&rutile, &mut cx), 0);
}

/// Edits for an AI client (ai-integration design §5.3) are undo steps like
/// the user's, and the undo and redo buttons say whose they are.
#[gpui_kit::test]
fn ai_edits_say_whose_they_are(cx: &mut TestAppContext) {
    use mw_crystal::{Agent, CellValues, Edit};

    let (workspace, mut cx) = open(cx, SampleKey::Rutile);
    let tooltips = |workspace: &Entity<StructureWorkspace>, cx: &mut VisualTestContext| {
        workspace.read_with(cx, |w, _| {
            (w.undo_tooltip().to_string(), w.redo_tooltip().to_string())
        })
    };
    let set_a = |a: f64| {
        Command::Edit(Edit::SetCell {
            values: CellValues::default().with(CellParam::A, a),
        })
    };
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.execute(set_a(4.6), window, cx).unwrap();
            let claude = Agent::new("Claude Code");
            workspace
                .execute_by(set_a(4.7), claude, window, cx)
                .unwrap();
        })
    });
    draw(&mut cx);
    assert_eq!(steps(&workspace, &mut cx), 2);
    let (undo, _) = tooltips(&workspace, &mut cx);
    assert_eq!(undo, "Undo Change cell parameters by AI (Claude Code)");
    let undone = cx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            w.undo_by(Some(Agent::new("Claude Code")), window, cx)
        })
    });
    assert!(undone.is_some());
    draw(&mut cx);
    let (undo, redo) = tooltips(&workspace, &mut cx);
    assert_eq!(undo, "Undo Change cell parameters");
    assert_eq!(redo, "Redo Change cell parameters by AI (Claude Code)");
    // A refused edit changes nothing and goes back to the caller.
    let refused = cx.update(|window, cx| {
        workspace.update(cx, |w, cx| {
            let edit = Command::Edit(Edit::RemoveSite { site: SiteId(99) });
            w.execute_by(edit, Agent::new("Claude Code"), window, cx)
        })
    });
    assert!(refused.is_err());
    assert_eq!(steps(&workspace, &mut cx), 1);
}
