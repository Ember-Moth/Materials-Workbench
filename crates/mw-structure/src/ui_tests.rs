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
    AppContext as _, Bounds, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext,
    WindowBounds, WindowOptions, point, px, size,
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
            let document = crate::document::tests::sample_document(key);
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
