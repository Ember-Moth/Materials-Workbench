//! One structure held for a script, worked on through the capability
//! registry.

use std::sync::{Arc, OnceLock};

use mw_automation::{
    Availability, CallError, CallResult, Documents, Effect, Host, Registry, StructureEntryId,
};
use mw_crystal::{CrystalDocument, Structure, SupercellError};
use mw_crystal_io::{ExportError, ImportedBlock, cif, poscar, xyz};
use serde_json::{Value, json};

use crate::format::Format;

/// Atoms a supercell may have. A structure this large takes minutes to
/// analyze and hundreds of megabytes to write; past it a typo such as
/// `200x200x200` is far likelier than intent.
pub const MAX_SUPERCELL_ATOMS: usize = 1_000_000;

/// Items the registry's paged capabilities return per call.
const PAGE: usize = 500;

/// The capability registry every script calls, `Registry::standard()`.
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Registry::standard)
}

/// `read_only`, `view`, `undoable`, `external` or `instrument`.
pub fn effect_name(effect: Effect) -> &'static str {
    match effect {
        Effect::ReadOnly => "read_only",
        Effect::View => "view",
        Effect::Undoable => "undoable",
        Effect::External => "external",
        Effect::Instrument => "instrument",
    }
}

/// Every capability of the registry: name, description, effect, where it
/// is available (`gui`, `headless` or `both`) and the JSON Schemas of its
/// input and output.
pub fn capabilities_json() -> Value {
    let capabilities: Vec<Value> = registry()
        .capabilities()
        .iter()
        .map(|capability| {
            let availability = match capability.availability {
                Availability::Gui => "gui",
                Availability::Headless => "headless",
                Availability::Both => "both",
            };
            json!({
                "name": capability.name,
                "description": capability.description,
                "effect": effect_name(capability.effect),
                "availability": availability,
                "input_schema": capability.input_schema,
                "output_schema": capability.output_schema,
            })
        })
        .collect();
    Value::Array(capabilities)
}

/// A structure held for a script: its document with the undo history,
/// its name, and the data block it was read from.
///
/// The document lives in an in-memory host (`mw_automation::Documents`) of
/// its own, so that capabilities run on it exactly as on a structure in the
/// application or the MCP server. Queries, the change of cell and
/// symmetrizing are capabilities; a supercell and file text come from the
/// domain functions the interface calls (`docs/design/scripting.md` §2).
#[derive(Clone, Debug)]
pub struct Document {
    host: Documents,
    id: StructureEntryId,
    name: String,
    /// The name of the file the structure came from, or the one its source
    /// structure came from; empty for none. It plays the part of the
    /// project's name in the interface.
    file: String,
}

/// File text written from a structure.
#[derive(Clone, Debug, PartialEq)]
pub struct Exported {
    pub format: Format,
    pub text: String,
    /// CIF only: tags of the source file left out because the structure
    /// changed since it was read.
    pub dropped: Vec<String>,
}

/// The cell a structure is described in after a change of cell
/// (crystallography design §7.8), as `structure.transform_cell` takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewCell {
    /// The conventional cell's centering removed.
    Primitive,
    /// The conventional cell of the standard setting.
    Conventional,
    /// The Niggli-reduced primitive cell.
    Niggli,
    /// Another tabulated setting of the same space group type, by its
    /// Hermann–Mauguin symbol, e.g. `P 1 1 21/a`.
    Setting(String),
    /// The change of cell (P, p) of International Tables A: the rows of P
    /// and the new origin p in the old basis, as fractions such as `1/2`;
    /// `None` keeps the origin. (P is boxed: nine strings would make every
    /// other choice as large.)
    Matrix {
        matrix: Box<[[String; 3]; 3]>,
        origin: Option<[String; 3]>,
    },
}

impl NewCell {
    /// `primitive`, `conventional` or `niggli`.
    pub fn preset(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "primitive" => Some(Self::Primitive),
            "conventional" => Some(Self::Conventional),
            "niggli" => Some(Self::Niggli),
            _ => None,
        }
    }

    /// The input of `structure.transform_cell`, less the structure.
    pub fn input(&self) -> Value {
        let (cell, setting, matrix, origin) = match self {
            Self::Primitive => ("primitive", "", json!([]), json!([])),
            Self::Conventional => ("conventional", "", json!([]), json!([])),
            Self::Niggli => ("niggli", "", json!([]), json!([])),
            Self::Setting(symbol) => ("setting", symbol.as_str(), json!([]), json!([])),
            Self::Matrix { matrix, origin } => {
                let origin = origin.as_ref().map_or_else(|| json!([]), |o| json!(o));
                ("matrix", "", json!(matrix), origin)
            }
        };
        json!({ "cell": cell, "setting": setting, "matrix": matrix, "origin": origin })
    }
}

impl Document {
    /// A structure that came from no file.
    pub fn new(name: impl Into<String>, structure: Structure) -> Self {
        Self::derived(name.into(), String::new(), structure)
    }

    /// A structure read from `block` of the file named `file`.
    pub(crate) fn imported(
        name: String,
        file: &str,
        structure: Structure,
        block: Arc<ImportedBlock>,
    ) -> Self {
        let mut host = Documents::new();
        let document = CrystalDocument::new(structure);
        let id = host.add_imported(&name, file, document, block);
        Self {
            host,
            id,
            name,
            file: file.to_string(),
        }
    }

    fn derived(name: String, file: String, structure: Structure) -> Self {
        let mut host = Documents::new();
        let id = host.add(&name, &file, CrystalDocument::new(structure));
        Self {
            host,
            id,
            name,
            file,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The name of the file the structure came from, directly or as the
    /// source of a supercell; empty for none.
    pub fn file(&self) -> &str {
        &self.file
    }

    /// The document: structure, derived data and undo history.
    pub fn crystal(&self) -> &CrystalDocument {
        self.host
            .document(self.id)
            .expect("the host holds the document")
    }

    pub fn structure(&self) -> &Structure {
        self.crystal().structure()
    }

    /// The data block the structure was read from, with the import
    /// diagnostics; `None` for a structure that came from no file.
    pub fn source(&self) -> Option<&ImportedBlock> {
        self.host.source(self.id)
    }

    /// Calls a capability of the registry on this structure. `input` is a
    /// JSON object or null; its `structure` is set to this one for the
    /// capabilities that take a structure.
    pub fn call(&mut self, capability: &str, input: Value) -> Result<CallResult, CallError> {
        let mut input = match input {
            Value::Null => json!({}),
            Value::Object(_) => input,
            _ => {
                return Err(CallError::new(
                    "invalid_arguments",
                    "the arguments must be a JSON object",
                ));
            }
        };
        let takes_structure = registry()
            .get(capability)
            .is_some_and(|c| c.input_schema["properties"].get("structure").is_some());
        if takes_structure {
            input["structure"] = json!(self.id);
        }
        registry().call(&mut self.host, capability, input)
    }

    /// `structure.summary`: formula, space group, cell, sites and atoms.
    pub fn summary(&mut self) -> Result<CallResult, CallError> {
        self.call("structure.summary", Value::Null)
    }

    /// `structure.sites`, every page.
    pub fn sites(&mut self) -> Result<CallResult, CallError> {
        self.all_pages("structure.sites", "sites")
    }

    /// `structure.diagnostics`, every page: what reading the file found.
    pub fn diagnostics(&mut self) -> Result<CallResult, CallError> {
        self.all_pages("structure.diagnostics", "diagnostics")
    }

    /// `structure.detect_symmetry`: the symmetry the atoms have, and what
    /// symmetrizing by it would do. `tolerance` is in Å, 0 for the
    /// structure's own.
    pub fn detect_symmetry(
        &mut self,
        tolerance: f64,
        standardize: bool,
    ) -> Result<CallResult, CallError> {
        let input = json!({ "tolerance": tolerance, "standardize": standardize });
        self.call("structure.detect_symmetry", input)
    }

    /// `structure.symmetrize`: symmetrizes the structure, undoably.
    pub fn symmetrize(
        &mut self,
        tolerance: f64,
        standardize: bool,
    ) -> Result<CallResult, CallError> {
        let input = json!({ "tolerance": tolerance, "standardize": standardize });
        self.call("structure.symmetrize", input)
    }

    /// `structure.transform_cell`: describes the structure in another
    /// cell, undoably; the atoms stay where they are.
    pub fn transform_cell(&mut self, cell: &NewCell) -> Result<CallResult, CallError> {
        self.call("structure.transform_cell", cell.input())
    }

    /// `history.undo`.
    pub fn undo(&mut self) -> Result<CallResult, CallError> {
        self.call("history.undo", Value::Null)
    }

    /// `history.redo`.
    pub fn redo(&mut self) -> Result<CallResult, CallError> {
        self.call("history.redo", Value::Null)
    }

    /// A supercell as a new structure in P1, as the interface makes it
    /// (`mw_crystal::make_supercell`, crystallography design §7.9): the
    /// columns of `matrix` are the new cell vectors in the old ones. This
    /// structure does not change. Named as the interface names it,
    /// `rutile 2×2×2 supercell`.
    pub fn supercell(&self, matrix: [[i32; 3]; 3]) -> Result<Document, CallError> {
        let determinant = determinant(matrix);
        let atoms = self.crystal().derived().atoms_in_cell() as i128 * i128::from(determinant);
        if atoms > MAX_SUPERCELL_ATOMS as i128 {
            return Err(CallError::new(
                "supercell_too_large",
                format!(
                    "the supercell would have {atoms} atoms, more than the \
                     {MAX_SUPERCELL_ATOMS} allowed"
                ),
            ));
        }
        let structure = mw_crystal::make_supercell(self.structure(), matrix).map_err(|error| {
            let code = match error {
                SupercellError::NotPositive { .. } => "supercell_not_positive",
            };
            CallError::new(code, error.to_string())
        })?;
        let name = match matrix {
            [[a, 0, 0], [0, b, 0], [0, 0, c]] => format!("{} {a}×{b}×{c} supercell", self.name),
            _ => format!("{} supercell", self.name),
        };
        Ok(Self::derived(name, self.file.clone(), structure))
    }

    /// The structure as file text, written as the interface's export
    /// dialog and `structure.export` write it, without their limit on
    /// size: a CIF of an imported structure keeps the source block's name
    /// and fields; the POSCAR comment line is `name · file`.
    pub fn export(&self, format: Format) -> Result<Exported, CallError> {
        let structure = self.structure();
        let (text, dropped) = match format {
            Format::Cif => {
                let options = match self.source() {
                    Some(block) => block.write_options(structure),
                    None => cif::WriteOptions::new(&self.name),
                };
                let written = cif::write(structure, &options);
                (written.text, written.dropped)
            }
            Format::Poscar => {
                let comment = if self.file.is_empty() {
                    self.name.clone()
                } else {
                    format!("{} · {}", self.name, self.file)
                };
                let text = poscar::write(structure, &comment).map_err(export_error)?;
                (text, Vec::new())
            }
            Format::Xyz => (xyz::write(structure).map_err(export_error)?, Vec::new()),
        };
        Ok(Exported {
            format,
            text,
            dropped,
        })
    }

    /// A paged capability's every page, as one result.
    fn all_pages(&mut self, capability: &str, list: &str) -> Result<CallResult, CallError> {
        let mut result = self.call(capability, json!({ "offset": 0, "limit": PAGE }))?;
        let total = result.output["total"].as_u64().unwrap_or(0) as usize;
        let mut offset = PAGE;
        while offset < total {
            let page = self.call(capability, json!({ "offset": offset, "limit": PAGE }))?;
            let Some(items) = page.output[list]
                .as_array()
                .filter(|items| !items.is_empty())
            else {
                break;
            };
            if let Some(all) = result.output[list].as_array_mut() {
                all.extend(items.iter().cloned());
            }
            offset += PAGE;
        }
        Ok(result)
    }
}

fn determinant(m: [[i32; 3]; 3]) -> i64 {
    let m = m.map(|row| row.map(i64::from));
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// The error `structure.export` gives (`mw-automation/src/files.rs`).
fn export_error(error: ExportError) -> CallError {
    let code = match &error {
        ExportError::PartialOccupancy { .. } => "partial_occupancy",
        ExportError::UnknownElement { .. } => "unknown_element",
        ExportError::NoAtoms => "no_atoms",
        _ => "not_exportable",
    };
    let hint = match &error {
        ExportError::PartialOccupancy { .. } => "; export as cif, which keeps occupancies",
        _ => "",
    };
    CallError::new(code, format!("{error}{hint}"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::read::read_file;

    fn fixture(name: &str) -> Document {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mw-crystal-io/tests/fixtures")
            .join(name);
        read_file(&path, None).unwrap().documents.remove(0)
    }

    /// The structure in a host of its own, as the MCP server and the
    /// application hold it, with the registry called directly.
    fn host(document: &Document) -> (Documents, StructureEntryId) {
        let mut host = Documents::new();
        let source = Arc::new(document.source().unwrap().clone());
        let id = host.add_imported(
            document.name(),
            document.file(),
            CrystalDocument::new(document.structure().clone()),
            source,
        );
        (host, id)
    }

    #[test]
    fn queries_are_the_registrys() {
        let mut rutile = fixture("rutile.cif");
        let (mut host, id) = host(&rutile);
        let direct = registry()
            .call(&mut host, "structure.summary", json!({ "structure": id }))
            .unwrap();
        assert_eq!(rutile.summary().unwrap(), direct);
        assert_eq!(direct.output["formula"], "O2Ti");

        // Errors too: rutile has no symmetry beyond its own.
        let input = json!({ "structure": id, "tolerance": 0.0, "standardize": false });
        let direct = registry()
            .call(&mut host, "structure.detect_symmetry", input)
            .unwrap_err();
        assert_eq!(direct.code, "symmetrize_no_higher_symmetry");
        assert_eq!(rutile.detect_symmetry(0.0, false).unwrap_err(), direct);
    }

    #[test]
    fn exports_match_structure_export() {
        for name in [
            "rutile.cif",
            "SrTiO3.cif",
            "zincite_legacy.cif",
            "bst_su_aniso.cif",
        ] {
            let document = fixture(name);
            let (mut host, id) = host(&document);
            for format in Format::ALL {
                let input = json!({ "structure": id, "format": format.name() });
                let direct = registry().call(&mut host, "structure.export", input);
                let exported = document.export(format);
                match (direct, exported) {
                    (Ok(direct), Ok(exported)) => {
                        assert_eq!(direct.output["text"], exported.text, "{name} {format}");
                        assert_eq!(direct.output["dropped_fields"], json!(exported.dropped));
                    }
                    (Err(direct), Err(exported)) => assert_eq!(direct, exported, "{name}"),
                    (direct, exported) => panic!("{name} {format}: {direct:?} {exported:?}"),
                }
            }
        }
    }

    #[test]
    fn supercells_are_the_interfaces() {
        let rutile = fixture("rutile.cif");
        let matrix = [[2, 0, 0], [0, 2, 0], [0, 0, 2]];
        let supercell = rutile.supercell(matrix).unwrap();
        assert_eq!(supercell.name(), "rutile 2×2×2 supercell");
        assert_eq!(supercell.file(), "rutile.cif");
        let direct = mw_crystal::make_supercell(rutile.structure(), matrix).unwrap();
        assert_eq!(supercell.structure(), &direct);
        assert_eq!(supercell.crystal().derived().atoms_in_cell(), 48);
        assert!(supercell.source().is_none());

        let skewed = rutile
            .supercell([[1, 1, 0], [-1, 1, 0], [0, 0, 1]])
            .unwrap();
        assert_eq!(skewed.name(), "rutile supercell");

        let flat = rutile
            .supercell([[1, 0, 0], [0, 1, 0], [0, 0, 0]])
            .unwrap_err();
        assert_eq!(flat.code, "supercell_not_positive");
        let huge = rutile
            .supercell([[100, 0, 0], [0, 100, 0], [0, 0, 100]])
            .unwrap_err();
        assert_eq!(huge.code, "supercell_too_large");
    }

    #[test]
    fn edits_go_through_the_undo_stack() {
        let mut corundum = fixture("corundum_hall.cif");
        let before = corundum.structure().clone();
        let result = corundum.transform_cell(&NewCell::Primitive).unwrap();
        assert_eq!(result.output["changed"], true);
        assert_ne!(corundum.structure(), &before);
        corundum.undo().unwrap();
        assert_eq!(corundum.structure(), &before);
        corundum.redo().unwrap();
        assert_ne!(corundum.structure(), &before);

        let matrix = NewCell::Matrix {
            matrix: Box::new([
                ["1".into(), "0".into(), "0".into()],
                ["0".into(), "1".into(), "0".into()],
                ["0".into(), "0".into(), "0".into()],
            ]),
            origin: None,
        };
        let refused = corundum.transform_cell(&matrix).unwrap_err();
        assert_eq!(refused.code, "transform_not_positive");
    }

    #[test]
    fn symmetrizing_finds_the_symmetry_of_p1_structures() {
        let mut halite = fixture("halite_p1.cif");
        let preview = halite.detect_symmetry(0.0, false).unwrap();
        assert_eq!(preview.output["detected"]["number"], 225);
        let done = halite.symmetrize(0.0, false).unwrap();
        assert_eq!(done.output["result"], preview.output);
        let summary = halite.summary().unwrap();
        assert_eq!(summary.output["space_group"]["number"], 225);
    }

    #[test]
    fn every_page_is_collected() {
        let rutile = fixture("rutile.cif");
        let mut big = rutile.supercell([[9, 0, 0], [0, 9, 0], [0, 0, 9]]).unwrap();
        let sites = big.sites().unwrap();
        assert_eq!(sites.output["total"], 6 * 729);
        assert_eq!(sites.output["sites"].as_array().unwrap().len(), 6 * 729);
    }

    #[test]
    fn calls_take_objects() {
        let mut rutile = fixture("rutile.cif");
        let wrong = rutile.call("structure.summary", json!([1])).unwrap_err();
        assert_eq!(wrong.code, "invalid_arguments");
        let unknown = rutile.call("structure.nothing", Value::Null).unwrap_err();
        assert_eq!(unknown.code, "unknown_capability");
        // The structure given is replaced by this one.
        let summary = rutile
            .call("structure.summary", json!({ "structure": 99 }))
            .unwrap();
        assert_eq!(summary.output["name"], "rutile");
    }
}
