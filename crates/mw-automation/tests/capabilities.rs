//! The standard capabilities: their schemas are in the strict subset model
//! vendors accept, and calls do what their descriptions say, with inputs
//! and outputs that match the schemas.

use mw_automation::{CallError, CallResult, Documents, Effect, Registry, StructureEntryId};
use mw_crystal::CrystalDocument;
use mw_crystal::sample::{self, SampleKey};
use mw_llm::schema::{check_schema, check_tools, validate_arguments};
use mw_llm::{ToolCall, ToolDefinition};
use serde_json::{Value, json};

fn session() -> (Registry, Documents, [StructureEntryId; 3]) {
    let mut documents = Documents::new();
    let mut add = |name: &str, key| {
        let sample = sample::load(key);
        documents.add(
            name,
            "Samples",
            CrystalDocument::with_derived(sample.structure, sample.derived),
        )
    };
    let ids = [
        add("Rutile", SampleKey::Rutile),
        add("SrTiO3", SampleKey::StrontiumTitanate),
        add("Rutile 2×2×2", SampleKey::RutileSupercell),
    ];
    (Registry::standard(), documents, ids)
}

fn conforms(schema: &Value, value: &Value, what: &str) {
    let call = ToolCall::new("check", "check", value.to_string());
    if let Err(error) = validate_arguments(schema, &call) {
        panic!("{what} does not match its schema: {error}\n{value:#}");
    }
}

/// Calls a capability, checking the arguments against the input schema and
/// a successful result against the output schema.
fn call(
    registry: &Registry,
    documents: &mut Documents,
    name: &str,
    input: Value,
) -> Result<CallResult, CallError> {
    let capability = registry.get(name).expect("registered");
    conforms(&capability.input_schema, &input, &format!("{name} input"));
    let result = registry.call(documents, name, input);
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

fn atom(site: u32, member: u16) -> Value {
    json!({ "site": site, "member": member, "cell": [0, 0, 0] })
}

#[test]
fn schemas_are_in_the_strict_subset() {
    let registry = Registry::standard();
    let mut tools = Vec::new();
    for capability in registry.capabilities() {
        let name = capability.name;
        assert!(
            name.split('.').count() == 2
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'.' || b == b'_'),
            "{name}"
        );
        assert!(capability.description.len() > 40, "{name}");
        for (which, schema) in [
            ("input", &capability.input_schema),
            ("output", &capability.output_schema),
        ] {
            if let Err(issues) = check_schema(schema) {
                panic!("{name} {which}: {issues:?}");
            }
        }
        // Inputs avoid union types: a request may only hold 16 of them.
        let stats = check_schema(&capability.input_schema).unwrap();
        assert_eq!(stats.union_parameters, 0, "{name}");
        // Tool names cannot contain dots; the assistant maps them.
        tools.push(ToolDefinition::new(
            name.replace('.', "__"),
            capability.description,
            capability.input_schema.clone(),
        ));
    }
    // All of them fit in one strict request.
    check_tools(&tools, true).unwrap();
}

#[test]
fn effects() {
    let registry = Registry::standard();
    let effect = |name| registry.get(name).unwrap().effect;
    assert_eq!(effect("structure.summary"), Effect::ReadOnly);
    assert_eq!(effect("structure.delete_sites"), Effect::Undoable);
    assert_eq!(effect("history.undo"), Effect::Undoable);
}

#[test]
fn queries() {
    let (registry, mut documents, [rutile, srtio3, _]) = session();
    let list = call(&registry, &mut documents, "structure.list", json!({})).unwrap();
    assert_eq!(list.output["structures"][1]["name"], "SrTiO3");
    // No arguments at all is the same as {}.
    assert!(
        registry
            .call(&mut documents, "structure.list", Value::Null)
            .is_ok()
    );

    let summary = call(
        &registry,
        &mut documents,
        "structure.summary",
        json!({ "structure": rutile }),
    )
    .unwrap();
    let output = &summary.output;
    assert_eq!(output["formula"], "O2Ti");
    assert_eq!(output["formula_units"], 2);
    assert_eq!(output["edit_mode"], "symmetry");
    assert_eq!(output["space_group"]["number"], 136);
    assert_eq!(output["cell"]["fixed"], json!(["alpha", "beta", "gamma"]));
    assert_eq!(
        output["cell"]["linked"],
        json!([{ "parameter": "b", "equals": "a" }])
    );
    assert!(
        summary.summary.contains("P 4_2/m n m (No. 136)"),
        "{}",
        summary.summary
    );

    let sites = call(
        &registry,
        &mut documents,
        "structure.sites",
        json!({ "structure": rutile, "offset": 0, "limit": 10 }),
    )
    .unwrap();
    assert_eq!(sites.output["total"], 2);
    assert_eq!(sites.output["sites"][1]["wyckoff"], "4f");
    assert_eq!(sites.output["sites"][1]["coordinate_form"], "x, x, 0");
    assert_eq!(sites.output["sites"][1]["occupants"][0]["label"], "O1");

    let symmetry = call(
        &registry,
        &mut documents,
        "structure.site_symmetry",
        json!({ "structure": rutile, "site": 2 }),
    )
    .unwrap();
    let coordinates = &symmetry.output["coordinates"];
    assert_eq!(coordinates[1]["constraint"], "follows");
    assert_eq!(coordinates[2]["constraint"], "fixed");
    assert_eq!(symmetry.output["wyckoff"]["site_symmetry"], "m.2m");

    let distance = call(
        &registry,
        &mut documents,
        "structure.measure_distance",
        json!({ "structure": srtio3, "atoms": [atom(2, 0), atom(3, 0)] }),
    )
    .unwrap();
    let value = distance.output["distance"].as_f64().unwrap();
    assert!((value - 3.905 / 2.0).abs() < 1e-9);

    let angle = call(
        &registry,
        &mut documents,
        "structure.measure_angle",
        json!({ "structure": srtio3, "atoms": [atom(3, 0), atom(2, 0), atom(3, 1)] }),
    )
    .unwrap();
    assert!((angle.output["angle"].as_f64().unwrap() - 90.0).abs() < 1e-9);

    let neighbors = call(
        &registry,
        &mut documents,
        "structure.neighbors",
        json!({ "structure": srtio3, "atom": atom(2, 0), "cutoff": 2.0 }),
    )
    .unwrap();
    assert_eq!(neighbors.output["neighbors"].as_array().unwrap().len(), 6);
    assert_eq!(neighbors.output["neighbors"][0]["element"], "O");
    assert_eq!(neighbors.output["truncated"], false);
}

#[test]
fn queries_refuse_bad_arguments() {
    let (registry, mut documents, [rutile, ..]) = session();
    let mut error =
        |name: &str, input: Value| registry.call(&mut documents, name, input).unwrap_err();
    assert_eq!(
        error("structure.summary", json!({ "structure": 9 })).code,
        "unknown_structure"
    );
    assert_eq!(
        error("structure.nothing", json!({})).code,
        "unknown_capability"
    );
    assert_eq!(
        error("structure.summary", json!({})).code,
        "invalid_arguments"
    );
    assert_eq!(
        error(
            "structure.summary",
            json!({ "structure": rutile, "extra": 1 })
        )
        .code,
        "invalid_arguments"
    );
    let too_many = json!({ "structure": rutile, "offset": 0, "limit": 501 });
    assert_eq!(error("structure.sites", too_many).code, "invalid_arguments");
    let one_atom = json!({ "structure": rutile, "atoms": [atom(1, 0)] });
    assert_eq!(
        error("structure.measure_distance", one_atom).code,
        "invalid_arguments"
    );
    let missing = json!({ "structure": rutile, "atoms": [atom(1, 0), atom(1, 5)] });
    assert_eq!(
        error("structure.measure_distance", missing).code,
        "unknown_atom"
    );
    let far = json!({ "structure": rutile, "atom": atom(1, 0), "cutoff": 50.0 });
    assert_eq!(error("structure.neighbors", far).code, "invalid_arguments");
}

#[test]
fn cell_edits_follow_the_symmetry() {
    let (registry, mut documents, [rutile, ..]) = session();
    let set = |documents: &mut Documents, values: Value| {
        call(
            &registry,
            documents,
            "structure.set_cell",
            json!({ "structure": rutile, "values": values }),
        )
    };
    let result = set(&mut documents, json!([{ "parameter": "a", "value": 4.7 }])).unwrap();
    assert_eq!(result.output["changed"], true);
    assert!(
        result.summary.contains("b = 4.7000 Å"),
        "{}",
        result.summary
    );

    let error = set(
        &mut documents,
        json!([{ "parameter": "gamma", "value": 100.0 }]),
    )
    .unwrap_err();
    assert_eq!(error.code, "fixed_cell_parameter");
    let error = set(
        &mut documents,
        json!([{ "parameter": "a", "value": 4.0 }, { "parameter": "a", "value": 4.1 }]),
    )
    .unwrap_err();
    assert_eq!(error.code, "invalid_arguments");

    let undo = call(
        &registry,
        &mut documents,
        "history.undo",
        json!({ "structure": rutile }),
    )
    .unwrap();
    assert_eq!(undo.output["command"], "set_cell");
    let summary = call(
        &registry,
        &mut documents,
        "structure.summary",
        json!({ "structure": rutile }),
    )
    .unwrap();
    assert_eq!(summary.output["cell"]["a"], 4.594);
}

#[test]
fn replacing_one_atom_under_symmetry_is_refused_with_guidance() {
    let (registry, mut documents, [rutile, ..]) = session();
    // One of the two Ti atoms.
    let one = json!({
        "structure": rutile, "occupant": "Ti1", "element": "Zr", "atoms": [atom(1, 1)],
    });
    let error = call(&registry, &mut documents, "structure.set_element", one).unwrap_err();
    assert_eq!(error.code, "breaks_symmetry");
    assert_eq!(
        error.to_json()["guidance"],
        json!([{ "kind": "whole_site", "site": 1 }, { "kind": "convert_to_p1" }])
    );

    // The whole orbit; the element's case does not matter.
    let all = json!({ "structure": rutile, "occupant": "Ti1", "element": "zr", "atoms": [] });
    let result = call(&registry, &mut documents, "structure.set_element", all).unwrap();
    assert_eq!(result.output["sites"], json!([1]));
    assert!(
        result.summary.contains("all 2 equivalent atoms"),
        "{}",
        result.summary
    );
    let summary = |documents: &mut Documents| {
        call(
            &registry,
            documents,
            "structure.summary",
            json!({ "structure": rutile }),
        )
        .unwrap()
        .output["formula"]
            .clone()
    };
    assert_eq!(summary(&mut documents), "O2Zr");

    call(
        &registry,
        &mut documents,
        "history.undo",
        json!({ "structure": rutile }),
    )
    .unwrap();
    assert_eq!(summary(&mut documents), "O2Ti");
    let redo = call(
        &registry,
        &mut documents,
        "history.redo",
        json!({ "structure": rutile }),
    )
    .unwrap();
    assert_eq!(redo.output["command"], "set_species");
    assert_eq!(summary(&mut documents), "O2Zr");
    let nothing = call(
        &registry,
        &mut documents,
        "history.redo",
        json!({ "structure": rutile }),
    )
    .unwrap();
    assert_eq!(
        nothing.output,
        json!({ "changed": false, "revision": 3, "command": null })
    );

    let unknown = json!({ "structure": rutile, "occupant": "Ti1", "element": "Qq", "atoms": [] });
    let error = call(&registry, &mut documents, "structure.set_element", unknown).unwrap_err();
    assert_eq!(error.code, "unknown_element");
}

#[test]
fn mixed_occupancy() {
    let (registry, mut documents, [rutile, ..]) = session();
    let added = call(
        &registry,
        &mut documents,
        "structure.add_occupant",
        json!({ "structure": rutile, "site": 1, "element": "Nb", "occupancy": 0.1 }),
    )
    .unwrap();
    assert!(added.summary.starts_with("Added Nb1"), "{}", added.summary);
    assert_eq!(added.output["warnings"][0]["code"], "occupancy_sum");

    let lowered = call(
        &registry,
        &mut documents,
        "structure.set_occupancy",
        json!({ "structure": rutile, "occupant": "Ti1", "occupancy": 0.9, "atoms": [] }),
    )
    .unwrap();
    assert_eq!(lowered.output["warnings"], json!([]));
    let sites = call(
        &registry,
        &mut documents,
        "structure.sites",
        json!({ "structure": rutile, "offset": 0, "limit": 1 }),
    )
    .unwrap();
    let occupants = &sites.output["sites"][0]["occupants"];
    assert_eq!(
        occupants[1],
        json!({ "label": "Nb1", "element": "Nb", "symbol": "Nb", "occupancy": 0.1 })
    );

    let renamed = call(
        &registry,
        &mut documents,
        "structure.rename_label",
        json!({ "structure": rutile, "occupant": "Nb1", "label": "Nb_a" }),
    )
    .unwrap();
    assert_eq!(renamed.output["changed"], true);
    call(
        &registry,
        &mut documents,
        "structure.remove_occupant",
        json!({ "structure": rutile, "occupant": "Nb_a" }),
    )
    .unwrap();
    let last = call(
        &registry,
        &mut documents,
        "structure.remove_occupant",
        json!({ "structure": rutile, "occupant": "Ti1" }),
    )
    .unwrap_err();
    assert_eq!(last.code, "last_occupant");
    assert_eq!(last.guidance.len(), 1);
}

#[test]
fn deleting_sites() {
    let (registry, mut documents, [_, srtio3, supercell]) = session();
    let delete = |documents: &mut Documents, structure, sites: Value, atoms: Value| {
        call(
            &registry,
            documents,
            "structure.delete_sites",
            json!({ "structure": structure, "sites": sites, "atoms": atoms }),
        )
    };
    let error = delete(&mut documents, srtio3, json!([]), json!([atom(3, 1)])).unwrap_err();
    assert_eq!(error.code, "breaks_symmetry");
    assert!(error.message.contains("1 of the 3"), "{}", error.message);
    let error = delete(&mut documents, srtio3, json!([]), json!([])).unwrap_err();
    assert_eq!(error.code, "invalid_arguments");

    let all_oxygen = json!([atom(3, 0), atom(3, 1), atom(3, 2)]);
    let deleted = delete(&mut documents, srtio3, json!([1]), all_oxygen).unwrap();
    assert_eq!(deleted.output["sites"], json!([1, 3]));
    assert_eq!(deleted.summary, "Deleted Sr1 (1 atom), O1 (3 atoms).");
    let sites = call(
        &registry,
        &mut documents,
        "structure.sites",
        json!({ "structure": srtio3, "offset": 0, "limit": 10 }),
    )
    .unwrap();
    assert_eq!(sites.output["total"], 1);

    // In P1 every atom is its own site.
    let one = delete(&mut documents, supercell, json!([]), json!([atom(7, 0)])).unwrap();
    assert_eq!(one.output["sites"], json!([7]));
}

#[test]
fn moving_atoms_only_in_p1() {
    let (registry, mut documents, [rutile, _, supercell]) = session();
    let refused = call(
        &registry,
        &mut documents,
        "structure.set_position",
        json!({ "structure": rutile, "site": 2, "position": [0.3, 0.3, 0.0] }),
    )
    .unwrap_err();
    assert_eq!(refused.code, "move_under_symmetry");
    assert_eq!(
        refused.to_json()["guidance"],
        json!([{ "kind": "convert_to_p1" }])
    );

    let moved = call(
        &registry,
        &mut documents,
        "structure.set_position",
        json!({ "structure": supercell, "site": 1, "position": [0.25, 0.25, 0.25] }),
    )
    .unwrap();
    // Onto site 2: allowed, with a warning.
    assert_eq!(moved.output["warnings"][0]["code"], "short_contact");
    assert!(moved.summary.contains("Warning"), "{}", moved.summary);
}
