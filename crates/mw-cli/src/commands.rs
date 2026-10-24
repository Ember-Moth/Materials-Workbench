//! The commands: each reads its files, works on every structure through
//! `mw-scripting`, and reports per structure.

use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use mw_automation::{Effect, diagnostic_message};
use mw_crystal_io::{Diagnostic, ImportedBlock, Severity};
use mw_scripting::{
    Document, Exported, FileReport, Format, ReadError, capabilities_json, diagnostic_json,
    diagnostic_line, failure_text, read_bytes, read_file, registry, severity_name,
};
use serde_json::{Value, json};

use crate::args::{CellArgs, Inputs, Output, SupercellSize, SymmetryArgs};
use crate::report::{Failure, Report, usage};

/// Errors of a file that could not be imported shown at most.
const MAX_DETAILS: usize = 10;

/// A file that was read.
struct Input {
    /// As the user named it; `stdin` for `-`.
    file: String,
    read: FileReport,
}

/// Reads every file. Files that cannot be read are reported as failures;
/// so are blocks that cannot be imported and files without structures,
/// with `report_failures` (the diagnostics command shows them itself).
fn read_inputs(inputs: &Inputs, report: &mut Report, report_failures: bool) -> Vec<Input> {
    let mut read = Vec::new();
    for path in &inputs.files {
        let (file, result) = if path.as_os_str() == "-" {
            let Some(format) = inputs.from else {
                usage("reading stdin (-) needs --from cif or --from poscar");
            };
            let mut bytes = Vec::new();
            if let Err(error) = std::io::stdin().read_to_end(&mut bytes) {
                report.error(Failure::new(
                    "stdin",
                    "unreadable",
                    format!("cannot read: {error}"),
                ));
                continue;
            }
            ("stdin".to_string(), read_bytes(&bytes, format, None))
        } else {
            (path.display().to_string(), read_file(path, inputs.from))
        };
        let file_report = match result {
            Ok(file_report) => file_report,
            Err(error) => {
                // The failure names the file already.
                let message = match &error {
                    ReadError::Io { error, .. } => format!("cannot read it: {error}"),
                    ReadError::UnknownFormat { .. } => "cannot tell the format from the name: \
                        .cif is read as CIF; .vasp, .poscar and names that start with POSCAR or \
                        CONTCAR as POSCAR; --from cif or --from poscar reads it as either"
                        .into(),
                    error => error.to_string(),
                };
                report.error(Failure::new(&file, error.code(), message));
                continue;
            }
        };
        if report_failures {
            for block in file_report.failed_blocks() {
                report.error(import_failure(&file, block));
            }
            if file_report.documents.is_empty() && file_report.failed_blocks().next().is_none() {
                let mut failure =
                    Failure::new(&file, "no_structure", "the file holds no structure");
                failure.details = file_report
                    .report
                    .all_diagnostics()
                    .map(diagnostic_line)
                    .collect();
                report.error(failure);
            }
        }
        read.push(Input {
            file,
            read: file_report,
        });
    }
    read
}

fn import_failure(file: &str, block: &ImportedBlock) -> Failure {
    let (heading, mut details) = failure_text(block, MAX_DETAILS);
    if let Some(last) = details.last_mut().filter(|last| last.starts_with('…')) {
        last.push_str("; `mw diagnostics` lists them all");
    }
    let mut failure = Failure::new(file, "import_failed", heading);
    failure.details = details;
    failure.diagnostics = block.diagnostics.iter().map(diagnostic_json).collect();
    failure
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The documents of every file, with the file they came from.
fn documents(inputs: &mut [Input]) -> impl Iterator<Item = (&str, &mut Document)> {
    inputs.iter_mut().flat_map(|input| {
        let file = input.file.as_str();
        input
            .read
            .documents
            .iter_mut()
            .map(move |document| (file, document))
    })
}

fn count(inputs: &[Input]) -> usize {
    inputs.iter().map(|input| input.read.documents.len()).sum()
}

pub fn info(inputs: &Inputs, report: &mut Report) {
    let mut read = read_inputs(inputs, report, true);
    let mut first = true;
    for (file, document) in documents(&mut read) {
        let name = document.name().to_string();
        match document.summary() {
            Ok(result) => {
                if !first {
                    report.out("\n");
                }
                first = false;
                report.out(&info_text(file, &name, &result.output));
                report.result(json!({
                    "file": file,
                    "structure": name,
                    "summary": result.summary,
                    "output": result.output,
                }));
            }
            Err(error) => report.error(Failure::call(file, &name, &error)),
        }
    }
}

/// The summary as a table, from `structure.summary`'s output.
fn info_text(file: &str, name: &str, o: &Value) -> String {
    let group = |sg: &Value| format!("{} (No. {})", text(&sg["symbol"]), sg["number"]);
    let mut out = format!("{file}: {name}\n");
    let mut row = |label: &str, value: String| out.push_str(&format!("  {label:<17}{value}\n"));
    row(
        "formula",
        format!("{}, Z = {}", text(&o["formula"]), o["formula_units"]),
    );
    let space_group = match &o["space_group"] {
        Value::Null => "not identified".to_string(),
        sg if sg["setting"] == "standard" => group(sg),
        sg => format!("{}, setting {}", group(sg), text(&sg["setting"])),
    };
    row("space group", space_group);
    if !o["detected_space_group"].is_null() {
        row(
            "atoms' symmetry",
            format!(
                "{}; `mw symmetrize` applies it",
                group(&o["detected_space_group"])
            ),
        );
    }
    let cell = &o["cell"];
    let number = |key: &str| cell[key].as_f64().unwrap_or(f64::NAN);
    row(
        "cell",
        format!(
            "a = {:.4} Å, b = {:.4} Å, c = {:.4} Å",
            number("a"),
            number("b"),
            number("c")
        ),
    );
    row(
        "",
        format!(
            "α = {:.3}°, β = {:.3}°, γ = {:.3}°",
            number("alpha"),
            number("beta"),
            number("gamma")
        ),
    );
    row("volume", format!("{:.3} Å³", number("volume")));
    if let Some(density) = o["density"].as_f64() {
        row("density", format!("{density:.3} g/cm³"));
    }
    let size = |key: &str| o[key].as_u64().unwrap_or(0) as usize;
    row(
        "contents",
        format!(
            "{}, {} in the cell, {}",
            plural(size("sites"), "site"),
            plural(size("atoms_in_cell"), "atom"),
            plural(size("operations"), "symmetry operation")
        ),
    );
    let mode = if o["edit_mode"] == "p1" {
        "P1"
    } else {
        "symmetry"
    };
    row("edit mode", mode.into());
    out
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

pub fn diagnostics(inputs: &Inputs, report: &mut Report) {
    let read = read_inputs(inputs, report, false);
    for input in &read {
        let file_report = &input.read;
        let mut out = format!("{}\n", input.file);
        for diagnostic in &file_report.report.diagnostics {
            out.push_str(&format!("  {}\n", diagnostic_row(diagnostic)));
        }
        for (block, document) in file_report.blocks() {
            let count = |s| {
                block
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity() == s)
                    .count()
            };
            let findings: Vec<String> = [
                (count(Severity::Error), "error", ""),
                (count(Severity::Warning), "warning", ""),
                (count(Severity::Info), "change", " by the importer"),
            ]
            .into_iter()
            .filter(|(count, ..)| *count > 0)
            .map(|(count, noun, tail)| plural(count, noun) + tail)
            .collect();
            let findings = if findings.is_empty() {
                "no findings".to_string()
            } else {
                findings.join(", ")
            };
            match document {
                Some(document) => out.push_str(&format!(
                    "  {} (block {:?}): {findings}\n",
                    document.name(),
                    block.name
                )),
                None => out.push_str(&format!(
                    "  block {:?} (line {}), not imported: {findings}\n",
                    block.name, block.line
                )),
            }
            for diagnostic in &block.diagnostics {
                out.push_str(&format!("    {}\n", diagnostic_row(diagnostic)));
            }
        }
        if file_report.documents.is_empty() && file_report.failed_blocks().next().is_none() {
            out.push_str("  no structure\n");
        }
        if file_report.failed() {
            report.fail();
        }
        report.out(&out);
        let mut result = json!({ "file": input.file });
        if let (Value::Object(result), Value::Object(found)) =
            (&mut result, file_report.diagnostics_json())
        {
            result.extend(found);
        }
        report.result(result);
    }
}

/// E.g. `warning  line 12  Tag '_x' has no value. (cif.missing_value)`.
fn diagnostic_row(diagnostic: &Diagnostic) -> String {
    let line = diagnostic
        .line()
        .map_or_else(String::new, |line| format!("line {line}"));
    format!(
        "{:<8} {line:<10} {} ({})",
        severity_name(diagnostic.severity()),
        diagnostic_message(diagnostic),
        diagnostic.code()
    )
}

pub fn symmetry(inputs: &Inputs, options: &SymmetryArgs, report: &mut Report) {
    let mut read = read_inputs(inputs, report, true);
    for (file, document) in documents(&mut read) {
        let name = document.name().to_string();
        let (summary, output) =
            match document.detect_symmetry(options.tolerance(), options.standardize) {
                Ok(result) => (result.summary, result.output),
                Err(error) if error.code == "symmetrize_no_higher_symmetry" => (
                    format!("No higher symmetry: {}.", error.message),
                    Value::Null,
                ),
                Err(error) => {
                    report.error(Failure::call(file, &name, &error));
                    continue;
                }
            };
        report.out(&format!("{file}: {name}\n  {summary}\n"));
        report.result(json!({
            "file": file,
            "structure": name,
            "summary": summary,
            "output": output,
        }));
    }
}

pub fn call(capability: &str, inputs: &Inputs, args: &str, report: &mut Report) {
    match registry().get(capability) {
        None => usage(&format!(
            "no capability named {capability}; `mw capabilities` lists them"
        )),
        Some(found) if found.effect != Effect::ReadOnly => usage(&format!(
            "{capability} changes the structure; mw call runs read-only capabilities, and \
             mw transform and mw symmetrize write changed structures"
        )),
        Some(_) => {}
    }
    let args: Value = match serde_json::from_str(args) {
        Ok(args @ Value::Object(_)) => args,
        _ => usage(&format!("--args {args:?} is not a JSON object")),
    };
    let mut read = read_inputs(inputs, report, true);
    for (file, document) in documents(&mut read) {
        let name = document.name().to_string();
        match document.call(capability, args.clone()) {
            Ok(result) => {
                report.out(&format!("{file}: {name}\n  {}\n", result.summary));
                report.result(json!({
                    "file": file,
                    "structure": name,
                    "summary": result.summary,
                    "output": result.output,
                }));
            }
            Err(error) => report.error(Failure::call(file, &name, &error)),
        }
    }
}

pub fn capabilities(report: &mut Report) {
    let mut out = String::new();
    for capability in registry().capabilities() {
        let effect = match capability.effect {
            Effect::ReadOnly => "read-only",
            Effect::View => "view",
            Effect::Undoable => "edit",
            Effect::External => "external",
            Effect::Instrument => "instrument",
        };
        let first = capability
            .description
            .split(". ")
            .next()
            .unwrap_or_default()
            .trim_end_matches('.');
        out.push_str(&format!("{:<28}{effect:<11}{first}.\n", capability.name));
    }
    report.out(&out);
    report.field("capabilities", capabilities_json());
}

/// Where written structures go, decided once the files are read.
struct Destination<'a> {
    output: &'a Output,
    format: Format,
    /// Paths written in this run, so that two structures never go to one.
    written: HashSet<PathBuf>,
}

/// The format to write: --to, else what --output's name tells, else
/// `default`. Decided before any file is read.
fn output_format(output: &Output, default: Option<Format>) -> Format {
    let named = output.output.as_deref().and_then(Format::of_output);
    let Some(format) = output.to.or(named).or(default) else {
        usage(
            "give the format to write with --to cif, poscar or xyz, or an --output name that tells it",
        );
    };
    format
}

impl<'a> Destination<'a> {
    /// Checks the output options against the number of structures.
    fn new(output: &'a Output, format: Format, structures: usize) -> Self {
        if output.output.is_some() && structures > 1 {
            usage(&format!(
                "--output takes one structure, but the files hold {structures}; use --out-dir"
            ));
        }
        if output.output.is_none() && output.out_dir.is_none() && structures > 1 {
            usage(&format!(
                "{structures} structures cannot all go to stdout; give --out-dir"
            ));
        }
        Self {
            output,
            format,
            written: HashSet::new(),
        }
    }

    /// Writes one structure: to stdout (or into the JSON), to --output, or
    /// into --out-dir as `stem` plus the format's extension. Reports what
    /// it did; `result` is the JSON result to complete.
    #[allow(clippy::too_many_arguments)]
    fn write(
        &mut self,
        report: &mut Report,
        file: &str,
        name: &str,
        stem: &str,
        exported: Exported,
        summary: &str,
        mut result: Value,
    ) {
        if !exported.dropped.is_empty() {
            report.note(&format!(
                "mw: warning: {file}: {name}: fields of the source left out because the \
                 structure changed: {}",
                exported.dropped.join(", ")
            ));
        }
        result["format"] = json!(self.format.name());
        result["dropped_fields"] = json!(exported.dropped);
        let path = match (&self.output.output, &self.output.out_dir) {
            (Some(path), _) => path.clone(),
            (None, Some(dir)) => {
                dir.join(format!("{}.{}", file_stem(stem), self.format.extension()))
            }
            (None, None) => {
                if report.json() {
                    result["path"] = Value::Null;
                    result["text"] = json!(exported.text);
                } else {
                    report.out(&exported.text);
                    if !summary.is_empty() {
                        report.note(&format!("{file}: {name}: {summary}"));
                    }
                }
                report.result(result);
                return;
            }
        };
        if let Err(failure) = self.write_file(&path, &exported.text) {
            report.error(Failure::new(file, failure.0, failure.1).structure(name));
            return;
        }
        let summary = if summary.is_empty() {
            String::new()
        } else {
            format!("{summary} ")
        };
        report.note(&format!("{file}: {name}: {summary}→ {}", path.display()));
        result["path"] = json!(path.display().to_string());
        result["text"] = Value::Null;
        report.result(result);
    }

    fn write_file(&mut self, path: &Path, text: &str) -> Result<(), (&'static str, String)> {
        if !self.written.insert(path.to_path_buf()) {
            return Err((
                "same_output",
                format!(
                    "another structure was written to {} already; their names give the same file",
                    path.display()
                ),
            ));
        }
        if path.exists() && !self.output.force {
            return Err((
                "exists",
                format!("{} exists; --force replaces it", path.display()),
            ));
        }
        if let Some(dir) = &self.output.out_dir {
            std::fs::create_dir_all(dir).map_err(|error| {
                (
                    "unwritable",
                    format!("cannot create {}: {error}", dir.display()),
                )
            })?;
        }
        std::fs::write(path, text).map_err(|error| {
            (
                "unwritable",
                format!("cannot write {}: {error}", path.display()),
            )
        })
    }
}

/// A file name for a structure: what its name says, without the
/// characters file systems refuse. `Si-relax · CONTCAR` becomes
/// `Si-relax-CONTCAR`.
fn file_stem(name: &str) -> String {
    let name = name.replace(" · ", "-");
    let stem: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() || c.is_whitespace() => '_',
            c => c,
        })
        .collect();
    let stem = stem.trim_matches(|c| c == '.' || c == '_');
    if stem.is_empty() {
        "structure".into()
    } else {
        stem.into()
    }
}

fn export(
    report: &mut Report,
    file: &str,
    document: &Document,
    format: Format,
) -> Option<Exported> {
    match document.export(format) {
        Ok(exported) => Some(exported),
        Err(error) => {
            report.error(Failure::call(file, document.name(), &error));
            None
        }
    }
}

fn written_result(file: &str, document: &Document) -> Value {
    json!({
        "file": file,
        "structure": document.name(),
        "atoms_in_cell": document.crystal().derived().atoms_in_cell(),
    })
}

pub fn convert(inputs: &Inputs, output: &Output, report: &mut Report) {
    let format = output_format(output, None);
    let mut read = read_inputs(inputs, report, true);
    let mut destination = Destination::new(output, format, count(&read));
    for (file, document) in documents(&mut read) {
        let Some(exported) = export(report, file, document, format) else {
            continue;
        };
        let mut result = written_result(file, document);
        result["summary"] = Value::Null;
        result["output"] = Value::Null;
        let name = document.name().to_string();
        destination.write(report, file, &name, &name, exported, "", result);
    }
}

pub fn supercell(inputs: &Inputs, size: &SupercellSize, output: &Output, report: &mut Report) {
    let format = output_format(output, Some(Format::Cif));
    let mut read = read_inputs(inputs, report, true);
    let mut destination = Destination::new(output, format, count(&read));
    let matrix = size.matrix();
    let suffix = match matrix {
        [[a, 0, 0], [0, b, 0], [0, 0, c]] => format!("-{a}x{b}x{c}"),
        _ => "-supercell".into(),
    };
    for (file, document) in documents(&mut read) {
        let supercell = match document.supercell(matrix) {
            Ok(supercell) => supercell,
            Err(error) => {
                report.error(Failure::call(file, document.name(), &error));
                continue;
            }
        };
        let Some(exported) = export(report, file, &supercell, format) else {
            continue;
        };
        let atoms = supercell.crystal().derived().atoms_in_cell();
        let summary = format!("{}, {}", supercell.name(), plural(atoms, "atom"));
        let mut result = written_result(file, &supercell);
        result["source"] = json!(document.name());
        result["matrix"] = json!(matrix);
        result["summary"] = json!(summary);
        result["output"] = Value::Null;
        let stem = format!("{}{suffix}", document.name());
        destination.write(
            report,
            file,
            document.name(),
            &stem,
            exported,
            &summary,
            result,
        );
    }
}

pub fn transform(inputs: &Inputs, cell: &CellArgs, output: &Output, report: &mut Report) {
    let format = output_format(output, Some(Format::Cif));
    let mut read = read_inputs(inputs, report, true);
    let mut destination = Destination::new(output, format, count(&read));
    let new_cell = cell.new_cell();
    for (file, document) in documents(&mut read) {
        let name = document.name().to_string();
        let result = match document.transform_cell(&new_cell) {
            Ok(result) => result,
            Err(error) => {
                report.error(Failure::call(file, &name, &error));
                continue;
            }
        };
        let Some(exported) = export(report, file, document, format) else {
            continue;
        };
        let mut json = written_result(file, document);
        json["summary"] = json!(result.summary);
        json["output"] = result.output;
        let stem = format!("{name}{}", cell.suffix());
        destination.write(report, file, &name, &stem, exported, &result.summary, json);
    }
}

pub fn symmetrize(inputs: &Inputs, options: &SymmetryArgs, output: &Output, report: &mut Report) {
    let format = output_format(output, Some(Format::Cif));
    let mut read = read_inputs(inputs, report, true);
    let mut destination = Destination::new(output, format, count(&read));
    for (file, document) in documents(&mut read) {
        let name = document.name().to_string();
        // A structure that already has all its atoms' symmetry is written
        // as it is: in a batch, that is no failure.
        let (summary, output) = match document.symmetrize(options.tolerance(), options.standardize)
        {
            Ok(result) => (result.summary, result.output),
            Err(error) if error.code == "symmetrize_no_higher_symmetry" => (
                format!("No higher symmetry: {}; written unchanged.", error.message),
                Value::Null,
            ),
            Err(error) => {
                report.error(Failure::call(file, &name, &error));
                continue;
            }
        };
        let Some(exported) = export(report, file, document, format) else {
            continue;
        };
        let mut json = written_result(file, document);
        json["summary"] = json!(summary);
        json["output"] = output;
        let stem = format!("{name}-symmetrized");
        destination.write(report, file, &name, &stem, exported, &summary, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_stems_keep_what_file_systems_allow() {
        assert_eq!(file_stem("rutile"), "rutile");
        assert_eq!(file_stem("Si-relax · CONTCAR"), "Si-relax-CONTCAR");
        assert_eq!(
            file_stem("金红石 2×2×2 supercell"),
            "金红石_2×2×2_supercell"
        );
        assert_eq!(file_stem("a/b:c"), "a_b_c");
        assert_eq!(file_stem("..."), "structure");
        assert_eq!(file_stem(""), "structure");
    }
}
