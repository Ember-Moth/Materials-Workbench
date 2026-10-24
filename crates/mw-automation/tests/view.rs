//! The view tools (ai-integration design §5.3): they check their arguments
//! and hand the host a change of what its window shows, or ask it for a
//! picture; they need a window, and no permission, and change no data.

use mw_automation::{
    CallError, Client, Context, DisplayModel, Documents, Effect, Host, Image, Permissions,
    Registry, StructureEntryId, StructureInfo, ViewChange,
};
use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{Agent, AtomRef, Command, CrystalDocument, Outcome, SiteId, Target};
use mw_project_file::{AuditEffect, AuditRecord, ProjectId};
use serde_json::json;

/// Documents shown in a window, which records what it was asked to show.
struct Window {
    documents: Documents,
    changes: Vec<(StructureEntryId, ViewChange)>,
    snapshots: Vec<(u32, u32)>,
}

impl Host for Window {
    fn structures(&self) -> Vec<StructureInfo> {
        self.documents.structures()
    }
    fn document(&self, id: StructureEntryId) -> Option<&CrystalDocument> {
        self.documents.document(id)
    }
    fn execute(
        &mut self,
        id: StructureEntryId,
        command: Command,
        agent: Option<Agent>,
    ) -> Result<Outcome, CallError> {
        self.documents.execute(id, command, agent)
    }
    fn undo(
        &mut self,
        id: StructureEntryId,
        _: Option<Agent>,
    ) -> Result<Option<Outcome>, CallError> {
        self.documents.undo(id, None)
    }
    fn redo(
        &mut self,
        id: StructureEntryId,
        _: Option<Agent>,
    ) -> Result<Option<Outcome>, CallError> {
        self.documents.redo(id, None)
    }
    fn has_window(&self) -> bool {
        true
    }
    fn audit(&mut self, project: Option<ProjectId>, record: AuditRecord) {
        self.documents.audit(project, record);
    }
    fn change_view(&mut self, id: StructureEntryId, change: ViewChange) -> Result<(), CallError> {
        self.changes.push((id, change));
        Ok(())
    }
    fn snapshot(
        &mut self,
        _: StructureEntryId,
        width: u32,
        height: u32,
    ) -> Result<Image, CallError> {
        self.snapshots.push((width, height));
        let (width, height) = if width == 0 {
            (800, 600)
        } else {
            (width, height)
        };
        Ok(Image::new(b"\x89PNG".to_vec(), width, height))
    }
}

fn window() -> (Window, StructureEntryId) {
    let mut documents = Documents::new();
    let sample = sample::load(SampleKey::Rutile);
    let id = documents.add(
        "Rutile",
        "Samples",
        CrystalDocument::with_derived(sample.structure, sample.derived),
    );
    let window = Window {
        documents,
        changes: Vec::new(),
        snapshots: Vec::new(),
    };
    (window, id)
}

/// A client the user allowed nothing.
fn stranger() -> Context {
    Context::client(
        Client::new("stranger", None),
        Permissions {
            modify_data: false,
            write_files: false,
        },
    )
}

#[test]
fn view_tools_change_only_the_view() {
    let registry = Registry::standard();
    for name in [
        "view.select",
        "view.set_display_model",
        "view.reset",
        "view.snapshot",
    ] {
        let capability = registry.get(name).unwrap();
        assert_eq!(capability.effect, Effect::View, "{name}");
        assert!(capability.idempotent && !capability.additive, "{name}");
    }

    let (mut window, id) = window();
    // Ti1 is site 1, O1 site 2.
    let ti = AtomRef::new(SiteId(1), 1, [1, 0, 0]);
    let selected = registry
        .call_as(
            &mut window,
            &stranger(),
            "view.select",
            json!({
                "structure": id,
                "sites": [2, 2],
                "atoms": [{ "site": 1, "member": 1, "cell": [1, 0, 0] }],
            }),
        )
        .unwrap();
    assert_eq!(selected.output, json!({ "sites": [2, 1], "atoms": 1 }));
    assert!(selected.summary.contains("O1, Ti1"), "{}", selected.summary);
    let model = json!({ "structure": id, "model": "polyhedra" });
    registry
        .call_as(&mut window, &stranger(), "view.set_display_model", model)
        .unwrap();
    let reset = json!({ "structure": id });
    registry
        .call_as(&mut window, &stranger(), "view.reset", reset)
        .unwrap();
    assert_eq!(
        window.changes,
        [
            (
                id,
                ViewChange::Select(vec![Target::Site(SiteId(2)), Target::Atom(ti)])
            ),
            (id, ViewChange::DisplayModel(DisplayModel::Polyhedra)),
            (id, ViewChange::ResetCamera),
        ]
    );

    let picture = registry
        .call_as(
            &mut window,
            &stranger(),
            "view.snapshot",
            json!({ "structure": id, "width": 0, "height": 0 }),
        )
        .unwrap();
    assert_eq!(picture.output, json!({ "width": 800, "height": 600 }));
    assert_eq!(picture.image.unwrap().width, 800);
    // No data changed, and every call is in the log.
    assert_eq!(window.document(id).unwrap().revision(), 0);
    let log = window.documents.audit_log();
    assert_eq!(log.len(), 4);
    assert!(
        log.iter()
            .all(|r| r.effect == AuditEffect::View && r.result.ok)
    );
}

#[test]
fn view_tools_check_their_arguments() {
    let registry = Registry::standard();
    let (mut window, id) = window();
    let mut call = |name: &str, input| {
        registry
            .call_as(&mut window, &stranger(), name, input)
            .unwrap_err()
            .code
    };
    let select = |sites, atoms| json!({ "structure": id, "sites": sites, "atoms": atoms });
    assert_eq!(
        call("view.select", select(json!([9]), json!([]))),
        "unknown_site"
    );
    let far = json!([{ "site": 1, "member": 7, "cell": [0, 0, 0] }]);
    assert_eq!(call("view.select", select(json!([]), far)), "unknown_atom");
    let model = json!({ "structure": id, "model": "wireframe" });
    assert_eq!(call("view.set_display_model", model), "invalid_arguments");
    for (width, height) in [(32, 600), (800, 4096), (0, 600)] {
        let size = json!({ "structure": id, "width": width, "height": height });
        assert_eq!(call("view.snapshot", size), "invalid_arguments");
    }
    let missing = json!({ "structure": 99 });
    assert_eq!(call("view.reset", missing), "unknown_structure");
    assert!(window.changes.is_empty() && window.snapshots.is_empty());
}

#[test]
fn without_a_window_there_is_no_view() {
    let registry = Registry::standard();
    let mut documents = Documents::new();
    let sample = sample::load(SampleKey::Rutile);
    let id = documents.add(
        "Rutile",
        "Samples",
        CrystalDocument::with_derived(sample.structure, sample.derived),
    );
    let reset = json!({ "structure": id });
    let error = registry
        .call(&mut documents, "view.reset", reset)
        .unwrap_err();
    assert_eq!(error.code, "unavailable");
}
