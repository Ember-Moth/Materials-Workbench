//! The project capabilities on a host with projects: listing and opening
//! projects, listing and opening their structures, what the other
//! structure capabilities say about a structure that is not open yet, and
//! making supercells into the project. The host here is a stand-in held in
//! memory; the MCP server's, which reads project files, is tested in
//! `mw-mcp`.

use std::path::PathBuf;

use mw_automation::{
    CallError, CallResult, Client, Context, Documents, EntryInfo, Host, Opening, Permissions,
    ProjectInfo, Projects, RecentProject, RecentProjects, Registry, StructureEntryId,
    StructureInfo, resource_uri, supercell_name,
};
use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{Agent, Command, CrystalDocument, Outcome, Structure};
use mw_llm::ToolCall;
use mw_llm::schema::validate_arguments;
use mw_project_file::{
    AuditRecord, Derived, EntryId, Import, Operation, ProjectId, Provenance, Sha256, SourceFormat,
    Timestamp, structure_sha256,
};
use serde_json::{Value, json};

const PROJECT: ProjectId = ProjectId::from_u128(0xa1);
const RUTILE: EntryId = EntryId::from_u128(0xb1);
const SUPERCELL: EntryId = EntryId::from_u128(0xb2);
const PATH: &str = "/projects/tio2.mwproj";

struct Entry {
    info: EntryInfo,
    structure: Structure,
    document: Option<CrystalDocument>,
}

/// A host with one project on disk, `PATH`, that it opens on request.
struct TestHost {
    open: bool,
    entries: Vec<Entry>,
    audit: Vec<(Option<ProjectId>, AuditRecord)>,
}

impl TestHost {
    fn new() -> Self {
        let rutile = sample::load(SampleKey::Rutile).structure;
        let supercell = sample::load(SampleKey::RutileSupercell).structure;
        let import = Import::new(
            SourceFormat::Cif,
            "rutile.cif",
            None,
            Sha256::of(b"rutile"),
            "rutile",
            Timestamp::from_unix(0),
            structure_sha256(&rutile),
        );
        let derived = Derived::new(
            RUTILE,
            "Rutile",
            structure_sha256(&rutile),
            Operation::MakeSupercell {
                matrix: [[2, 0, 0], [0, 2, 0], [0, 0, 2]],
            },
            Timestamp::from_unix(0),
        );
        let entry = |id, entry, name: &str, provenance, structure| Entry {
            info: EntryInfo::new(
                StructureEntryId(id),
                PROJECT,
                entry,
                name,
                provenance,
                false,
            ),
            structure,
            document: None,
        };
        Self {
            open: false,
            audit: Vec::new(),
            entries: vec![
                entry(1, RUTILE, "Rutile", Provenance::Import(import), rutile),
                entry(
                    2,
                    SUPERCELL,
                    "Rutile 2×2×2",
                    Provenance::Derived(derived),
                    supercell,
                ),
            ],
        }
    }

    fn entry(&self, id: StructureEntryId) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| self.open && e.info.structure == id)
    }

    fn document_mut(&mut self, id: StructureEntryId) -> Result<&mut CrystalDocument, CallError> {
        self.entries
            .iter_mut()
            .find(|e| e.info.structure == id)
            .and_then(|e| e.document.as_mut())
            .ok_or_else(|| CallError::new("unknown_structure", "no such structure"))
    }
}

impl Host for TestHost {
    fn structures(&self) -> Vec<StructureInfo> {
        self.entries
            .iter()
            .filter(|e| e.document.is_some())
            .map(|e| StructureInfo::new(e.info.structure, &e.info.name, "TiO2"))
            .collect()
    }

    fn document(&self, id: StructureEntryId) -> Option<&CrystalDocument> {
        self.entry(id)?.document.as_ref()
    }

    fn execute(
        &mut self,
        id: StructureEntryId,
        command: Command,
        agent: Option<Agent>,
    ) -> Result<Outcome, CallError> {
        Ok(self.document_mut(id)?.execute_by(command, agent)?)
    }

    fn undo(&mut self, id: StructureEntryId) -> Result<Option<Outcome>, CallError> {
        Ok(self.document_mut(id)?.undo())
    }

    fn redo(&mut self, id: StructureEntryId) -> Result<Option<Outcome>, CallError> {
        Ok(self.document_mut(id)?.redo())
    }

    fn projects(&self) -> Option<&dyn Projects> {
        Some(self)
    }

    fn projects_mut(&mut self) -> Option<&mut dyn Projects> {
        Some(self)
    }

    fn audit(&mut self, project: Option<ProjectId>, record: AuditRecord) {
        self.audit.push((project, record));
    }
}

impl Projects for TestHost {
    fn projects(&self) -> Vec<ProjectInfo> {
        if self.open {
            vec![ProjectInfo::new(PROJECT, "TiO2", Some(PathBuf::from(PATH)))]
        } else {
            Vec::new()
        }
    }

    fn recent_projects(&self) -> RecentProjects {
        RecentProjects {
            projects: vec![RecentProject::new(
                PROJECT,
                "TiO2",
                PathBuf::from(PATH),
                Timestamp::from_unix(0),
            )],
            withheld: 2,
        }
    }

    fn entries(&self) -> Vec<EntryInfo> {
        if !self.open {
            return Vec::new();
        }
        self.entries
            .iter()
            .map(|e| {
                let mut info = e.info.clone();
                info.open = e.document.is_some();
                info
            })
            .collect()
    }

    fn open_project(&mut self, path: &str) -> Result<(ProjectId, Opening), CallError> {
        if path != PATH {
            return Err(CallError::new("not_found", format!("{path} is no project")));
        }
        let opening = if self.open {
            Opening::AlreadyOpen
        } else {
            Opening::Opened
        };
        self.open = true;
        Ok((PROJECT, opening))
    }

    fn open_structure(&mut self, id: StructureEntryId) -> Result<Opening, CallError> {
        let open = self.open;
        let entry = self
            .entries
            .iter_mut()
            .find(|e| open && e.info.structure == id)
            .ok_or_else(|| CallError::new("unknown_structure", "no such structure"))?;
        if entry.document.is_some() {
            return Ok(Opening::AlreadyOpen);
        }
        entry.document = Some(CrystalDocument::new(entry.structure.clone()));
        Ok(Opening::Opened)
    }

    fn add_derived(
        &mut self,
        from: StructureEntryId,
        name: Option<&str>,
        structure: Structure,
        matrix: [[i32; 3]; 3],
    ) -> Result<StructureEntryId, CallError> {
        let source = self
            .entries
            .iter()
            .position(|e| e.info.structure == from)
            .ok_or_else(|| CallError::new("unknown_structure", "no such structure"))?;
        let from_name = self.entries[source].info.name.clone();
        let name = name.map_or_else(|| supercell_name(&from_name, matrix), str::to_owned);
        let derived = Derived::new(
            self.entries[source].info.entry,
            from_name,
            structure_sha256(&self.entries[source].structure),
            Operation::MakeSupercell { matrix },
            Timestamp::from_unix(0),
        );
        let next = self
            .entries
            .iter()
            .map(|e| e.info.structure.0)
            .max()
            .unwrap()
            + 1;
        let id = StructureEntryId(next);
        let info = EntryInfo::new(
            id,
            PROJECT,
            EntryId::new(),
            name,
            Provenance::Derived(derived),
            true,
        );
        let document = Some(CrystalDocument::new(structure.clone()));
        self.entries.insert(
            source + 1,
            Entry {
                info,
                structure,
                document,
            },
        );
        Ok(id)
    }
}

fn conforms(schema: &Value, value: &Value, what: &str) {
    let call = ToolCall::new("check", "check", value.to_string());
    if let Err(error) = validate_arguments(schema, &call) {
        panic!("{what} does not match its schema: {error}\n{value:#}");
    }
}

fn call(host: &mut dyn Host, name: &str, input: Value) -> Result<CallResult, CallError> {
    let registry = Registry::standard();
    let capability = registry.get(name).expect("registered");
    conforms(&capability.input_schema, &input, &format!("{name} input"));
    let result = registry.call(host, name, input);
    if let Ok(result) = &result {
        conforms(
            &capability.output_schema,
            &result.output,
            &format!("{name} output"),
        );
        assert!(!result.summary.is_empty());
    }
    result
}

#[test]
fn listing_and_opening_projects() {
    let mut host = TestHost::new();
    let list = call(&mut host, "project.list", json!({})).unwrap().output;
    assert_eq!(list["open"], json!([]));
    assert_eq!(list["recent"][0]["path"], PATH);
    assert_eq!(list["recent"][0]["open"], false);
    assert_eq!(list["recent"][0]["project"], PROJECT.to_string());
    assert_eq!(list["recent_elsewhere"], 2);

    let missing = call(&mut host, "project.open", json!({ "path": "/x.mwproj" }));
    assert_eq!(missing.unwrap_err().code, "not_found");

    let opened = call(&mut host, "project.open", json!({ "path": PATH })).unwrap();
    assert!(
        opened.summary.starts_with("Opened project TiO2"),
        "{}",
        opened.summary
    );
    let project = &opened.output;
    assert_eq!(project["name"], "TiO2");
    let structures = project["structures"].as_array().unwrap();
    assert_eq!(structures.len(), 2);
    assert_eq!(structures[0]["structure"], 1);
    assert_eq!(structures[0]["open"], false);
    assert_eq!(
        structures[0]["origin"],
        "imported from rutile.cif (data block rutile)"
    );
    assert_eq!(structures[1]["origin"], "2×2×2 supercell of Rutile");
    assert_eq!(
        structures[1]["resource"],
        format!("mw://project/{PROJECT}/structure/{SUPERCELL}.cif")
    );
    assert_eq!(structures[1]["resource"], resource_uri(PROJECT, SUPERCELL));

    let again = call(&mut host, "project.open", json!({ "path": PATH })).unwrap();
    assert!(
        again.summary.starts_with("Already open"),
        "{}",
        again.summary
    );
    assert_eq!(again.output, opened.output);

    let list = call(&mut host, "project.list", json!({})).unwrap().output;
    assert_eq!(list["open"][0]["structures"], 2);
    assert_eq!(list["open"][0]["path"], PATH);
    assert_eq!(list["recent"][0]["open"], true);
}

#[test]
fn structures_are_opened_before_use() {
    let mut host = TestHost::new();
    call(&mut host, "project.open", json!({ "path": PATH })).unwrap();

    // Listed, but not open: the error says how to open it.
    let summary = call(&mut host, "structure.summary", json!({ "structure": 1 }));
    let error = summary.unwrap_err();
    assert_eq!(error.code, "structure_not_open");
    assert!(
        error.message.contains("structure.open"),
        "{}",
        error.message
    );
    let edit = json!({ "structure": 1, "values": [{ "parameter": "a", "value": 4.7 }] });
    assert_eq!(
        call(&mut host, "structure.set_cell", edit)
            .unwrap_err()
            .code,
        "structure_not_open"
    );
    let unknown = call(&mut host, "structure.summary", json!({ "structure": 9 }));
    assert_eq!(unknown.unwrap_err().code, "unknown_structure");

    let opened = call(&mut host, "structure.open", json!({ "structure": 1 })).unwrap();
    let output = &opened.output;
    assert_eq!(output["name"], "Rutile");
    assert_eq!(output["project"], "TiO2");
    assert_eq!(output["resource"], resource_uri(PROJECT, RUTILE));
    // The copy of rutile.cif is not there: the result says what is missing.
    assert!(
        output["note"]
            .as_str()
            .is_some_and(|note| note.contains("rutile.cif")),
        "{output:#}"
    );
    let again = call(&mut host, "structure.open", json!({ "structure": 1 })).unwrap();
    assert!(
        again.summary.starts_with("Already open"),
        "{}",
        again.summary
    );

    let summary = call(&mut host, "structure.summary", json!({ "structure": 1 })).unwrap();
    assert_eq!(summary.output["formula"], "O2Ti");
    let list = call(&mut host, "structure.list", json!({})).unwrap();
    let open: Vec<bool> = list.output["structures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_object().unwrap()["open"].as_bool().unwrap())
        .collect();
    assert_eq!(open, [true, false]);
    assert_eq!(list.summary, "2 structures, 1 of them open.");

    // Files outside projects are for hosts that offer them.
    let file = call(&mut host, "file.open", json!({ "path": "/x/rutile.cif" }));
    assert_eq!(file.unwrap_err().code, "unavailable");
}

#[test]
fn hosts_without_projects() {
    let mut documents = Documents::new();
    let rutile = sample::load(SampleKey::Rutile);
    let id = documents.add(
        "Rutile",
        "Samples",
        CrystalDocument::with_derived(rutile.structure, rutile.derived),
    );
    for (name, input) in [
        ("project.list", json!({})),
        ("project.open", json!({ "path": PATH })),
        ("structure.open", json!({ "structure": id })),
        ("file.open", json!({ "path": "rutile.cif" })),
    ] {
        let error = call(&mut documents, name, input).unwrap_err();
        assert_eq!(error.code, "unavailable", "{name}");
    }
    // Their documents are listed, open, in no project file.
    let list = call(&mut documents, "structure.list", json!({})).unwrap();
    assert_eq!(
        list.output["structures"],
        json!([{
            "structure": id,
            "name": "Rutile",
            "project": "Samples",
            "origin": null,
            "open": true,
            "resource": null,
        }])
    );
}

#[test]
fn supercells_join_the_project() {
    let mut host = TestHost::new();
    call(&mut host, "project.open", json!({ "path": PATH })).unwrap();
    call(&mut host, "structure.open", json!({ "structure": 1 })).unwrap();
    let doubled = json!({
        "structure": 1,
        "matrix": [[2, 0, 0], [0, 2, 0], [0, 0, 2]],
        "name": "",
    });
    let made = call(&mut host, "structure.make_supercell", doubled).unwrap();
    assert_eq!(made.output["name"], "Rutile 2×2×2 supercell");
    assert_eq!(made.output["cells"], 8);
    assert_eq!(made.output["sites"], 48);
    let id = made.output["structure"].as_u64().unwrap();
    assert_eq!(id, 3);
    assert!(
        made.summary
            .starts_with("Made Rutile 2×2×2 supercell (id 3)"),
        "{}",
        made.summary
    );
    // After its source, open; the source is as it was.
    let list = call(&mut host, "structure.list", json!({})).unwrap();
    let names: Vec<&str> = list.output["structures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Rutile", "Rutile 2×2×2 supercell", "Rutile 2×2×2"]);
    let summary = call(&mut host, "structure.summary", json!({ "structure": id })).unwrap();
    assert_eq!(summary.output["atoms_in_cell"], 48);
    assert_eq!(summary.output["edit_mode"], "p1");
    assert_eq!(host.document(StructureEntryId(1)).unwrap().revision(), 0);

    let named =
        json!({ "structure": 1, "matrix": [[1, 1, 0], [-1, 1, 0], [0, 0, 1]], "name": "√2" });
    let made = call(&mut host, "structure.make_supercell", named).unwrap();
    assert_eq!(made.output["name"], "√2");
    assert_eq!(made.output["cells"], 2);

    let flat = json!({ "structure": 1, "matrix": [[1, 0, 0], [0, 1, 0], [0, 0, 0]], "name": "" });
    let refused = call(&mut host, "structure.make_supercell", flat).unwrap_err();
    assert_eq!(refused.code, "invalid_arguments");
    let huge =
        json!({ "structure": 1, "matrix": [[40, 0, 0], [0, 40, 0], [0, 0, 40]], "name": "" });
    let refused = call(&mut host, "structure.make_supercell", huge).unwrap_err();
    assert_eq!(refused.code, "too_large");
}

#[test]
fn client_calls_are_recorded_for_their_project() {
    let mut host = TestHost::new();
    let registry = Registry::standard();
    let claude = Client::new("claude-code", Some("Claude Code".into()));
    let context = Context::client(claude, Permissions::default());
    let run = |host: &mut TestHost, name: &str, input: Value| {
        registry.call_as(host, &context, name, input)
    };
    run(&mut host, "project.list", json!({})).unwrap();
    run(&mut host, "project.open", json!({ "path": PATH })).unwrap();
    run(&mut host, "structure.open", json!({ "structure": 1 })).unwrap();
    let edit = json!({ "structure": 1, "values": [{ "parameter": "a", "value": 4.7 }] });
    run(&mut host, "structure.set_cell", edit).unwrap();
    let refused = run(&mut host, "structure.summary", json!({ "structure": 2 }));
    assert_eq!(refused.unwrap_err().code, "structure_not_open");

    let records: Vec<(Option<ProjectId>, &str, Option<EntryId>, bool)> = host
        .audit
        .iter()
        .map(|(project, r)| (*project, r.capability.as_str(), r.structure, r.result.ok))
        .collect();
    assert_eq!(
        records,
        [
            (None, "project.list", None, true),
            (Some(PROJECT), "project.open", None, true),
            (Some(PROJECT), "structure.open", Some(RUTILE), true),
            (Some(PROJECT), "structure.set_cell", Some(RUTILE), true),
            (Some(PROJECT), "structure.summary", Some(SUPERCELL), false),
        ]
    );
    let (_, edit) = &host.audit[3];
    assert_eq!(edit.client.title.as_deref(), Some("Claude Code"));
    assert_eq!(edit.arguments["values"][0]["value"], 4.7);
    assert!(!edit.confirmed);
    // The edit is the client's step in the undo stack.
    let document = host.document(StructureEntryId(1)).unwrap();
    assert_eq!(document.undo_agent().map(Agent::name), Some("Claude Code"));
}
