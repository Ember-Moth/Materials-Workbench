//! Runs the `mw` executable on the test files of `mw-crystal-io` and checks
//! that it writes what the domain functions and the capability registry
//! give when called directly, the way the application calls them; and that
//! wrong input fails with a clear message and the right exit status.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

use mw_automation::{Documents, Registry, StructureEntryId};
use mw_crystal::{CrystalDocument, Structure};
use mw_crystal_io::{ImportReport, ImportedBlock, cif, poscar, xyz};
use serde_json::{Value, json};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mw-crystal-io/tests/fixtures")
        .canonicalize()
        .unwrap()
}

fn fixture(name: &str) -> String {
    fixtures().join(name).display().to_string()
}

fn mw(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mw"))
        .args(args)
        .output()
        .expect("mw runs")
}

fn mw_with_stdin(args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mw"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mw runs");
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// Succeeded, or says why not.
#[track_caller]
fn ok(output: &Output) {
    assert!(
        output.status.success(),
        "{:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        stdout(output),
        stderr(output)
    );
}

#[track_caller]
fn exit_code(output: &Output, code: i32, stderr_has: &[&str]) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "stderr:\n{}",
        stderr(output)
    );
    let text = stderr(output);
    for part in stderr_has {
        assert!(text.contains(part), "{part:?} not in stderr:\n{text}");
    }
}

fn json_output(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("JSON on stdout")
}

/// A structure read directly with `mw-crystal-io`, as the application
/// reads a file with one structure: named after the file, the file's own
/// diagnostics before its block's.
struct Direct {
    name: String,
    file: String,
    structure: Structure,
    block: Arc<ImportedBlock>,
}

fn direct(name: &str) -> Direct {
    let path = fixtures().join(name);
    let bytes = std::fs::read(&path).unwrap();
    let report: ImportReport = if name.ends_with(".cif") {
        cif::read(&bytes)
    } else {
        poscar::read(&bytes)
    };
    let (block, structure) = report.structures().next().expect("a structure");
    let mut block = block.clone();
    block
        .diagnostics
        .splice(0..0, report.diagnostics.iter().cloned());
    Direct {
        name: path.file_stem().unwrap().to_string_lossy().into_owned(),
        file: path.file_name().unwrap().to_string_lossy().into_owned(),
        structure: structure.clone(),
        block: Arc::new(block),
    }
}

impl Direct {
    /// The structure in a host, as the MCP server holds it.
    fn host(&self) -> (Documents, StructureEntryId) {
        let mut host = Documents::new();
        let id = host.add_imported(
            &self.name,
            &self.file,
            CrystalDocument::new(self.structure.clone()),
            self.block.clone(),
        );
        (host, id)
    }

    fn cif(&self, structure: &Structure) -> String {
        cif::write(structure, &self.block.write_options(structure)).text
    }

    fn poscar(&self, structure: &Structure) -> String {
        poscar::write(structure, &format!("{} · {}", self.name, self.file)).unwrap()
    }
}

/// Fixtures with one structure that every format can hold.
const ORDERED: [&str; 7] = [
    "rutile.cif",
    "SrTiO3.cif",
    "zincite_legacy.cif",
    "corundum_hall.cif",
    "poscar/si_diamond.vasp",
    "poscar/gaas_primitive_cartesian.vasp",
    "poscar/zno_left_handed.vasp",
];

#[test]
fn info_is_structure_summary() {
    let registry = Registry::standard();
    for name in ORDERED.iter().chain(&["halite_p1.cif", "bst_su_aniso.cif"]) {
        let output = mw(&["info", "--json", &fixture(name)]);
        ok(&output);
        let result = &json_output(&output)["results"][0];
        let direct = direct(name);
        let (mut host, id) = direct.host();
        let summary = registry
            .call(&mut host, "structure.summary", json!({ "structure": id }))
            .unwrap();
        assert_eq!(result["output"], summary.output, "{name}");
        assert_eq!(result["summary"], summary.summary, "{name}");
        assert_eq!(result["structure"], direct.name);
    }
    // The text names what matters.
    let output = mw(&["info", &fixture("rutile.cif")]);
    ok(&output);
    let text = stdout(&output);
    for part in [
        "O2Ti, Z = 2",
        "P 4_2/m n m (No. 136)",
        "a = 4.5940 Å",
        "6 atoms",
    ] {
        assert!(text.contains(part), "{part:?} not in\n{text}");
    }
}

#[test]
fn convert_writes_what_the_writers_write() {
    for name in ORDERED {
        let direct = direct(name);
        let structure = &direct.structure;
        let expected = [
            ("cif", direct.cif(structure)),
            ("poscar", direct.poscar(structure)),
            ("xyz", xyz::write(structure).unwrap()),
        ];
        for (format, text) in expected {
            let output = mw(&["convert", &fixture(name), "--to", format]);
            ok(&output);
            assert_eq!(stdout(&output), text, "{name} as {format}");
        }
    }
}

#[test]
fn convert_writes_a_batch_into_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().to_str().unwrap();
    let files = [
        fixture("rutile.cif"),
        fixture("SrTiO3.cif"),
        fixture("multi_block.cif"),
        fixture("poscar/CONTCAR"),
    ];
    let mut args = vec!["convert", "--to", "poscar", "--out-dir", out];
    args.extend(files.iter().map(String::as_str));
    let output = mw(&args);
    ok(&output);
    let mut written: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert!(written.contains(&"rutile.vasp".to_string()), "{written:?}");
    assert!(written.contains(&"SrTiO3.vasp".to_string()), "{written:?}");
    assert!(
        written.contains(&"poscar-CONTCAR.vasp".to_string()),
        "{written:?}"
    );
    assert!(
        written
            .iter()
            .filter(|w| w.starts_with("multi_block-"))
            .count()
            >= 2,
        "{written:?}"
    );
    let rutile = direct("rutile.cif");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("rutile.vasp")).unwrap(),
        rutile.poscar(&rutile.structure)
    );

    // Files are not replaced unless asked.
    std::fs::write(dir.path().join("rutile.vasp"), "kept").unwrap();
    let again = mw(&args);
    exit_code(&again, 1, &["rutile.vasp exists", "--force"]);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("rutile.vasp")).unwrap(),
        "kept"
    );
    args.push("--force");
    ok(&mw(&args));
    assert_ne!(
        std::fs::read_to_string(dir.path().join("rutile.vasp")).unwrap(),
        "kept"
    );
}

#[test]
fn output_files_tell_the_format() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("POSCAR");
    let output = mw(&[
        "convert",
        &fixture("rutile.cif"),
        "-o",
        path.to_str().unwrap(),
    ]);
    ok(&output);
    let rutile = direct("rutile.cif");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        rutile.poscar(&rutile.structure)
    );
}

#[test]
fn supercells_are_make_supercell() {
    let rutile = direct("rutile.cif");
    let diagonal = [[2, 0, 0], [0, 2, 0], [0, 0, 2]];
    let supercell = mw_crystal::make_supercell(&rutile.structure, diagonal).unwrap();
    let output = mw(&[
        "supercell",
        &fixture("rutile.cif"),
        "--repeat",
        "2x2x2",
        "--to",
        "poscar",
    ]);
    ok(&output);
    let comment = "rutile 2×2×2 supercell · rutile.cif";
    assert_eq!(stdout(&output), poscar::write(&supercell, comment).unwrap());

    let skewed = [[1, 1, 0], [-1, 1, 0], [0, 0, 1]];
    let supercell = mw_crystal::make_supercell(&rutile.structure, skewed).unwrap();
    let output = mw(&[
        "supercell",
        &fixture("rutile.cif"),
        "--matrix",
        "1 1 0; -1 1 0; 0 0 1",
    ]);
    ok(&output);
    let options = cif::WriteOptions::new("rutile supercell");
    assert_eq!(stdout(&output), cif::write(&supercell, &options).text);
}

#[test]
fn transform_is_structure_transform_cell() {
    let registry = Registry::standard();
    let cases: [(&[&str], Value); 3] = [
        (
            &["--cell", "primitive"],
            json!({ "cell": "primitive", "setting": "", "matrix": [], "origin": [] }),
        ),
        (
            &["--matrix", "1 0 0; 0 1 0; 0 0 1", "--origin", "0 0 1/4"],
            json!({
                "cell": "matrix",
                "setting": "",
                "matrix": [["1", "0", "0"], ["0", "1", "0"], ["0", "0", "1"]],
                "origin": ["0", "0", "1/4"],
            }),
        ),
        (
            &["--setting", "R -3 c :R"],
            json!({ "cell": "setting", "setting": "R -3 c :R", "matrix": [], "origin": [] }),
        ),
    ];
    for (args, input) in cases {
        let corundum = direct("corundum_hall.cif");
        let (mut host, id) = corundum.host();
        let mut input = input;
        input["structure"] = json!(id);
        let result = registry
            .call(&mut host, "structure.transform_cell", input)
            .unwrap();
        let structure = mw_automation::Host::document(&host, id)
            .unwrap()
            .structure();
        let mut command = vec!["transform", "--to", "cif"];
        let path = fixture("corundum_hall.cif");
        command.push(&path);
        command.extend(args);
        let output = mw(&command);
        ok(&output);
        assert_eq!(stdout(&output), corundum.cif(structure), "{args:?}");
        assert!(
            stderr(&output).contains(&result.summary),
            "{}",
            stderr(&output)
        );

        command.push("--json");
        let output = mw(&command);
        ok(&output);
        assert_eq!(json_output(&output)["results"][0]["output"], result.output);
    }
}

#[test]
fn symmetry_is_detect_symmetry_and_symmetrize() {
    let registry = Registry::standard();
    for name in ["halite_p1.cif", "poscar/CONTCAR"] {
        let direct = direct(name);
        let (mut host, id) = direct.host();
        let input = json!({ "structure": id, "tolerance": 0.0, "standardize": true });
        let preview = registry
            .call(&mut host, "structure.detect_symmetry", input.clone())
            .unwrap();
        let output = mw(&["symmetry", "--json", "--standardize", &fixture(name)]);
        ok(&output);
        assert_eq!(json_output(&output)["results"][0]["output"], preview.output);

        registry
            .call(&mut host, "structure.symmetrize", input)
            .unwrap();
        let structure = mw_automation::Host::document(&host, id)
            .unwrap()
            .structure();
        let output = mw(&["symmetrize", "--standardize", &fixture(name)]);
        ok(&output);
        assert_eq!(stdout(&output), direct.cif(structure), "{name}");
    }

    // No higher symmetry is an answer, not a failure.
    let output = mw(&["symmetry", &fixture("rutile.cif")]);
    ok(&output);
    assert!(stdout(&output).contains("No higher symmetry"));
    let output = mw(&["symmetrize", &fixture("rutile.cif")]);
    ok(&output);
    let rutile = direct("rutile.cif");
    assert_eq!(stdout(&output), rutile.cif(&rutile.structure));
}

#[test]
fn diagnostics_are_structure_diagnostics() {
    let registry = Registry::standard();
    for name in [
        "rutile_near_special.cif",
        "zincite_legacy.cif",
        "poscar/CONTCAR",
    ] {
        let direct = direct(name);
        let (mut host, id) = direct.host();
        let input = json!({ "structure": id, "offset": 0, "limit": 500 });
        let expected = registry
            .call(&mut host, "structure.diagnostics", input)
            .unwrap();
        let output = mw(&["diagnostics", "--json", &fixture(name)]);
        ok(&output);
        let result = &json_output(&output)["results"][0];
        let mut listed = result["diagnostics"].as_array().unwrap().clone();
        listed.extend(
            result["blocks"][0]["diagnostics"]
                .as_array()
                .unwrap()
                .clone(),
        );
        assert_eq!(
            Value::Array(listed),
            expected.output["diagnostics"],
            "{name}"
        );
    }

    let output = mw(&[
        "diagnostics",
        &fixture("malformed.cif"),
        &fixture("rutile.cif"),
    ]);
    exit_code(&output, 1, &[]);
    let text = stdout(&output);
    assert!(text.contains("not imported"), "{text}");
    assert!(text.contains("line 13"), "{text}");
    assert!(text.contains("no findings"), "{text}");
}

#[test]
fn stdin_is_read_with_from() {
    let bytes = std::fs::read(fixtures().join("rutile.cif")).unwrap();
    let output = mw_with_stdin(&["convert", "-", "--from", "cif", "--to", "poscar"], &bytes);
    ok(&output);
    let rutile = direct("rutile.cif");
    // Named after its data block, with no file.
    assert_eq!(
        stdout(&output),
        poscar::write(&rutile.structure, "rutile").unwrap()
    );
    let output = mw_with_stdin(&["convert", "-", "--to", "poscar"], &bytes);
    exit_code(&output, 2, &["--from"]);
}

#[test]
fn call_runs_read_only_capabilities() {
    let registry = Registry::standard();
    let rutile = direct("rutile.cif");
    let (mut host, id) = rutile.host();
    let input = json!({ "structure": id, "offset": 0, "limit": 10 });
    let expected = registry.call(&mut host, "structure.sites", input).unwrap();
    let output = mw(&[
        "call",
        "structure.sites",
        &fixture("rutile.cif"),
        "--args",
        r#"{"offset": 0, "limit": 10}"#,
        "--json",
    ]);
    ok(&output);
    assert_eq!(
        json_output(&output)["results"][0]["output"],
        expected.output
    );

    let edit = mw(&["call", "structure.set_cell", &fixture("rutile.cif")]);
    exit_code(&edit, 2, &["changes the structure"]);
    let unknown = mw(&["call", "structure.nothing", &fixture("rutile.cif")]);
    exit_code(&unknown, 2, &["no capability named structure.nothing"]);
    let bad = mw(&[
        "call",
        "structure.sites",
        &fixture("rutile.cif"),
        "--args",
        "[1]",
    ]);
    exit_code(&bad, 2, &["not a JSON object"]);
    // The capability's own error.
    let limit = mw(&[
        "call",
        "structure.sites",
        &fixture("rutile.cif"),
        "--args",
        "{}",
    ]);
    exit_code(&limit, 1, &["rutile", "offset"]);
}

#[test]
fn capabilities_are_the_registrys() {
    let output = mw(&["capabilities", "--json"]);
    ok(&output);
    let listed = json_output(&output)["capabilities"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(listed, Registry::standard().capabilities().len());
    let output = mw(&["capabilities"]);
    ok(&output);
    assert!(stdout(&output).contains("structure.detect_symmetry"));
}

#[test]
fn wrong_input_fails_clearly() {
    // Files that cannot be read, with the reason; the others still done.
    let output = mw(&["info", "missing.cif", "notes.txt", &fixture("rutile.cif")]);
    exit_code(
        &output,
        1,
        &[
            "missing.cif: cannot read it",
            "notes.txt: cannot tell the format",
            "--from",
        ],
    );
    assert!(stdout(&output).contains("O2Ti"));

    // Import errors with their lines.
    let output = mw(&["info", &fixture("malformed.cif")]);
    exit_code(
        &output,
        1,
        &["not imported", "line 13", "symmetry.invalid_operation"],
    );

    // What a format cannot hold.
    let output = mw(&["convert", &fixture("bst_su_aniso.cif"), "--to", "poscar"]);
    exit_code(&output, 1, &["partially occupied", "export as cif"]);

    // Usage: where several structures go, the format, the arguments.
    let two = [fixture("rutile.cif"), fixture("SrTiO3.cif")];
    let output = mw(&["convert", &two[0], &two[1], "--to", "cif"]);
    exit_code(&output, 2, &["cannot all go to stdout", "--out-dir"]);
    let output = mw(&["convert", &two[0], &two[1], "-o", "x.cif"]);
    exit_code(&output, 2, &["--output takes one structure"]);
    let output = mw(&["convert", &two[0]]);
    exit_code(&output, 2, &["--to"]);
    let output = mw(&["convert", &two[0], "--to", "pdb"]);
    exit_code(&output, 2, &["no format"]);
    let output = mw(&["info", &two[0], "--from", "xyz"]);
    exit_code(&output, 2, &["written, not read"]);
    let output = mw(&["supercell", &two[0], "--repeat", "2x2"]);
    exit_code(&output, 2, &["--repeat"]);
    let output = mw(&["supercell", &two[0]]);
    exit_code(&output, 2, &["--repeat"]);
    let output = mw(&[
        "transform",
        &two[0],
        "--cell",
        "primitive",
        "--setting",
        "P 1",
    ]);
    exit_code(&output, 2, &["cannot be used with"]);
    let output = mw(&[
        "transform",
        &two[0],
        "--origin",
        "0 0 1/2",
        "--cell",
        "primitive",
    ]);
    exit_code(&output, 2, &["--origin", "cannot be used with"]);
    let output = mw(&["transform", &two[0], "--origin", "0 0 1/2"]);
    exit_code(&output, 2, &["--matrix"]);
    let output = mw(&["info"]);
    exit_code(&output, 2, &["FILE"]);

    // Refused operations, with the reason.
    let output = mw(&["supercell", &two[0], "--matrix", "1 0 0; 0 1 0; 0 0 0"]);
    exit_code(&output, 1, &["determinant 0"]);
    let output = mw(&["transform", &two[0], "--matrix", "1 1 0; 0 1 0; 0 0 1/3"]);
    exit_code(&output, 1, &["rutile"]);

    // With --json, errors are data too.
    let output = mw(&["info", "--json", "missing.cif"]);
    exit_code(&output, 1, &[]);
    let document = json_output(&output);
    assert_eq!(document["errors"][0]["code"], "unreadable");
    assert_eq!(document["errors"][0]["file"], "missing.cif");
    let output = mw(&["info", "--json", &fixture("malformed.cif")]);
    let document = json_output(&output);
    assert_eq!(document["errors"][0]["code"], "import_failed");
    assert!(
        !document["errors"][0]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
