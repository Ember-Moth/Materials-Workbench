//! The standard capabilities: their schemas are in the strict subset model
//! vendors accept, and calls do what their descriptions say, with inputs
//! and outputs that match the schemas.

use std::sync::Arc;

use mw_automation::{CallError, CallResult, Documents, Effect, Registry, StructureEntryId};
use mw_crystal::CrystalDocument;
use mw_crystal::sample::{self, SampleKey};
use mw_crystal_io::cif;
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
        let tool = ToolDefinition::new(
            name.replace('.', "__"),
            capability.description,
            capability.input_schema.clone(),
        );
        tools.push((capability.effect, tool));
    }
    // A strict request holds at most 20 tools, fewer than there are
    // capabilities, so the assistant picks them by task (design §4). The
    // read-only ones, which the MCP server offers, fit in one request.
    let read_only: Vec<ToolDefinition> = tools
        .iter()
        .filter(|(effect, _)| *effect == Effect::ReadOnly)
        .map(|(_, tool)| tool.clone())
        .collect();
    check_tools(&read_only, true).unwrap();
    let tools: Vec<ToolDefinition> = tools.into_iter().map(|(_, tool)| tool).collect();
    for request in tools.chunks(20) {
        check_tools(request, true).unwrap();
    }
}

#[test]
fn effects() {
    let registry = Registry::standard();
    let effect = |name| registry.get(name).unwrap().effect;
    assert_eq!(effect("structure.summary"), Effect::ReadOnly);
    // Returns text; writing files would be External.
    assert_eq!(effect("structure.export"), Effect::ReadOnly);
    assert_eq!(effect("structure.delete_sites"), Effect::Undoable);
    // Write tools for MCP come with v0.3; the server offers none yet.
    for name in [
        "structure.move_site",
        "structure.set_coordinate",
        "structure.add_site",
    ] {
        assert_eq!(effect(name), Effect::Undoable, "{name}");
    }
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
fn moving_atoms() {
    let (registry, mut documents, [rutile, _, supercell]) = session();
    // Under symmetry, O1 snaps back onto 4f, x, x, 0, 0.0015 Å away.
    call(
        &registry,
        &mut documents,
        "structure.set_position",
        json!({ "structure": rutile, "site": 2, "position": [0.3, 0.3, 0.0005] }),
    )
    .unwrap();
    let sites = call(
        &registry,
        &mut documents,
        "structure.sites",
        json!({ "structure": rutile, "offset": 1, "limit": 1 }),
    )
    .unwrap();
    assert_eq!(sites.output["sites"][0]["position"], json!([0.3, 0.3, 0.0]));

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

fn site_position(
    registry: &Registry,
    documents: &mut Documents,
    structure: StructureEntryId,
    index: u32,
) -> Value {
    let sites = call(
        registry,
        documents,
        "structure.sites",
        json!({ "structure": structure, "offset": index, "limit": 1 }),
    )
    .unwrap();
    sites.output["sites"][0]["position"].clone()
}

#[test]
fn moving_sites_under_symmetry() {
    let (registry, mut documents, [rutile, ..]) = session();
    let mut move_site = |displacement: Value, leave: bool| {
        call(
            &registry,
            &mut documents,
            "structure.move_site",
            json!({
                "structure": rutile,
                "atom": atom(2, 0),
                "displacement": displacement,
                "leave_special_position": leave,
            }),
        )
    };
    // Along x, x, 0 only: half of the x displacement, none of z.
    let moved = move_site(json!([0.01, 0.0, 0.02]), false).unwrap();
    assert_eq!(moved.output["sites"], json!([2]));
    assert_eq!(moved.output["info"], json!([]));
    assert_eq!(
        moved.summary,
        "Moved O1 at (0.3098, 0.3098, 0.0000), 4 equivalent atoms (4f)."
    );
    // Off the mirror z = 0, still on the mirror x = y: 8j, with two
    // of its atoms 0.3 Å apart across z = 0.
    let left = move_site(json!([0.0, 0.0, 0.05]), true).unwrap();
    assert_eq!(left.output["info"][0]["code"], "multiplicity");
    assert_eq!(left.output["warnings"][0]["code"], "close_images");
    assert_eq!(
        left.summary,
        "Moved O1 at (0.3098, 0.3098, 0.0500), 8 equivalent atoms (8j). Note: site 2 now has \
         8 equivalent atoms in the cell instead of 4. Warning: equivalent atoms of site 2 are \
         0.296 Å apart."
    );
}

#[test]
fn setting_coordinates() {
    let (registry, mut documents, [rutile, ..]) = session();
    let mut set = |axis: &str, value: f64| {
        call(
            &registry,
            &mut documents,
            "structure.set_coordinate",
            json!({ "structure": rutile, "site": 2, "axis": axis, "value": value }),
        )
    };
    // y of x, x, 0 follows x: setting it sets x.
    let set_y = set("y", 0.3125).unwrap();
    assert_eq!(
        set_y.summary,
        "Set y = 0.3125: O1 at (0.3125, 0.3125, 0.0000), 4 equivalent atoms (4f)."
    );
    let fixed = set("z", 0.1).unwrap_err();
    assert_eq!(fixed.code, "fixed_coordinate");
    assert_eq!(fixed.message, "z of site 2 is fixed by its site symmetry");
    // Its own value changes nothing.
    assert_eq!(set("z", 0.0).unwrap().output["changed"], false);
    // x near 0 puts O1 on 2a, where Ti1 is.
    let snapped = set("x", 0.0001).unwrap();
    let codes: Vec<&Value> = snapped.output["info"]
        .as_array()
        .unwrap()
        .iter()
        .map(|info| &info["code"])
        .collect();
    assert_eq!(codes, [&json!("snapped"), &json!("multiplicity")]);
    assert_eq!(snapped.output["warnings"][0]["code"], "short_contact");
    assert_eq!(
        site_position(&registry, &mut documents, rutile, 1),
        json!([0.0, 0.0, 0.0])
    );

    let unknown_axis = registry.call(
        &mut documents,
        "structure.set_coordinate",
        json!({ "structure": rutile, "site": 2, "axis": "w", "value": 0.1 }),
    );
    assert_eq!(unknown_axis.unwrap_err().code, "invalid_arguments");
}

#[test]
fn adding_sites() {
    let (registry, mut documents, [rutile, ..]) = session();
    let mut add = |element: &str, position: Value| {
        call(
            &registry,
            &mut documents,
            "structure.add_site",
            json!({
                "structure": rutile,
                "element": element,
                "position": position,
                "occupancy": 1.0,
            }),
        )
    };
    // 0.003 Å off 4f: snapped onto it and reported.
    let added = add("o", json!([0.2046, 0.2050, 0.001])).unwrap();
    assert_eq!(added.output["sites"], json!([3]));
    assert_eq!(added.output["info"][0]["code"], "snapped");
    assert!(
        added
            .summary
            .starts_with("Added O2 at (0.2048, 0.2048, 0.0000), 4 equivalent atoms (4f)."),
        "{}",
        added.summary
    );
    // On an atom of O1: add an occupant to O1 instead.
    let refused = add("F", json!([0.6952, 0.6952, 0.0])).unwrap_err();
    assert_eq!(refused.code, "coincident_site");
    assert_eq!(
        refused.to_json()["guidance"],
        json!([{ "kind": "add_occupant", "site": 2 }])
    );
    assert_eq!(
        add("Zz", json!([0.5, 0.0, 0.0])).unwrap_err().code,
        "unknown_element"
    );

    let undone = call(
        &registry,
        &mut documents,
        "history.undo",
        json!({ "structure": rutile }),
    )
    .unwrap();
    assert!(undone.summary.contains("Undid"), "{}", undone.summary);
    let sites = call(
        &registry,
        &mut documents,
        "structure.sites",
        json!({ "structure": rutile, "offset": 0, "limit": 10 }),
    )
    .unwrap();
    assert_eq!(sites.output["total"], 2);
}

/// Ba and Sr on one position, operations from the H-M symbol and γ left
/// out: three changes the importer reports.
const MIXED: &str = "\
data_bst
_publ_section_title 'Ba/Sr titanate'
_cell_length_a 3.95
_cell_length_b 3.95
_cell_length_c 3.95
_cell_angle_alpha 90
_cell_angle_beta 90
_space_group_name_H-M_alt 'P m -3 m'
loop_
_atom_site_label
_atom_site_type_symbol
_atom_site_fract_x
_atom_site_fract_y
_atom_site_fract_z
_atom_site_occupancy
Ba1 Ba 0 0 0 0.6
Sr1 Sr 0 0 0 0.4
Ti1 Ti 0.5 0.5 0.5 1
O1 O 0.5 0.5 0 1
";

#[test]
fn import_diagnostics() {
    let (registry, mut documents, [rutile, ..]) = session();
    let page = |structure| json!({ "structure": structure, "offset": 0, "limit": 10 });
    let sample = call(
        &registry,
        &mut documents,
        "structure.diagnostics",
        page(rutile),
    )
    .unwrap();
    assert_eq!(sample.output["block"], Value::Null);
    assert_eq!(sample.output["total"], 0);

    let block = cif::read_str(MIXED).blocks.remove(0);
    let document = CrystalDocument::new(block.structure.clone().unwrap());
    let bst = documents.add_imported("bst", "Imported", document, Arc::new(block));
    let found = call(
        &registry,
        &mut documents,
        "structure.diagnostics",
        page(bst),
    )
    .unwrap();
    let output = &found.output;
    assert_eq!(output["block"], "bst");
    assert_eq!(
        (output["errors"].clone(), output["infos"].clone()),
        (json!(0), json!(3))
    );
    let codes: Vec<&str> = output["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        [
            "cell.angle_defaulted",
            "symmetry.operations_from_name",
            "atom.merged_occupancy"
        ]
    );
    let merged = &output["diagnostics"][2];
    assert_eq!(merged["line"], 17);
    assert_eq!(
        merged["message"],
        "Atom 'Sr1' is at the same position as 'Ba1'; they were merged into one site with \
         mixed occupancy."
    );
    assert!(
        found.summary.contains("3 changes by the importer"),
        "{}",
        found.summary
    );

    let second = json!({ "structure": bst, "offset": 1, "limit": 1 });
    let second = call(&registry, &mut documents, "structure.diagnostics", second).unwrap();
    assert_eq!(second.output["total"], 3);
    assert_eq!(
        second.output["diagnostics"][0]["code"],
        "symmetry.operations_from_name"
    );
    let refused = registry
        .call(
            &mut documents,
            "structure.diagnostics",
            json!({ "structure": bst, "offset": 0, "limit": 0 }),
        )
        .unwrap_err();
    assert_eq!(refused.code, "invalid_arguments");
}

#[test]
fn export_as_text() {
    let (registry, mut documents, [_, srtio3, _]) = session();
    let export = |documents: &mut Documents, structure, format| {
        let input = json!({ "structure": structure, "format": format });
        call(&registry, documents, "structure.export", input)
    };

    let poscar = export(&mut documents, srtio3, "poscar").unwrap();
    let lines: Vec<&str> = poscar.output["text"].as_str().unwrap().lines().collect();
    assert_eq!(lines[0], "SrTiO3 · Samples");
    assert_eq!(
        lines[6].split_whitespace().collect::<Vec<_>>(),
        ["1", "1", "3"]
    );
    assert_eq!(poscar.output["extension"], "vasp");
    let xyz = export(&mut documents, srtio3, "xyz").unwrap();
    assert_eq!(
        xyz.output["text"].as_str().unwrap().lines().next(),
        Some("5")
    );
    let cif = export(&mut documents, srtio3, "cif").unwrap();
    assert!(cif.output["text"].as_str().unwrap().contains("data_SrTiO3"));
    assert!(
        cif.summary.starts_with("CIF text of SrTiO3"),
        "{}",
        cif.summary
    );

    // An imported block keeps its fields; a list of atoms cannot hold
    // mixed occupancy.
    let block = cif::read_str(MIXED).blocks.remove(0);
    let document = CrystalDocument::new(block.structure.clone().unwrap());
    let bst = documents.add_imported("bst", "Imported", document, Arc::new(block));
    let cif = export(&mut documents, bst, "cif").unwrap();
    let text = cif.output["text"].as_str().unwrap();
    assert!(
        text.contains("data_bst") && text.contains("Ba/Sr titanate"),
        "{text}"
    );
    assert_eq!(cif.output["dropped_fields"], json!([]));
    let refused = export(&mut documents, bst, "poscar").unwrap_err();
    assert_eq!(refused.code, "partial_occupancy");
    assert!(
        refused.message.contains("export as cif"),
        "{}",
        refused.message
    );
}
