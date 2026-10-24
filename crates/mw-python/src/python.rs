//! The PyO3 bindings of `materials_workbench` (`docs/design/scripting.md`
//! §5). Results are the registry's JSON outputs as Python dicts and lists;
//! refusals raise `CapabilityError` with the registry's code, message and
//! guidance.

use std::ffi::CString;
use std::path::{Path, PathBuf};

use mw_automation::{CallError, CallResult};
use mw_scripting::{
    Document, FileReport, ReadError as ScriptReadError, capabilities_json, failure_text,
    read_bytes, read_file,
};
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyOSError, PyTypeError, PyUserWarning, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyString;
use serde_json::Value;

use crate::{Number, Size};

/// Errors of a file that could not be imported listed at most.
const MAX_DETAILS: usize = 10;

create_exception!(
    materials_workbench,
    WorkbenchError,
    PyException,
    "The base class of the errors this module raises."
);
create_exception!(
    materials_workbench,
    ReadError,
    WorkbenchError,
    "A file's format could not be told from its name, or it holds no structure that could \
     be imported. `diagnostics` holds what reading it found, as `diagnostics()` gives it."
);
create_exception!(
    materials_workbench,
    CapabilityError,
    WorkbenchError,
    "A capability refused the call; nothing changed. `code` is stable (e.g. \
     transform_not_positive), `message` says what was wrong and `guidance` what to do instead."
);

fn to_python<'py>(py: Python<'py>, value: &Value) -> PyResult<Bound<'py, PyAny>> {
    let text = serde_json::to_string(value).map_err(|e| PyValueError::new_err(e.to_string()))?;
    py.import("json")?.call_method1("loads", (text,))
}

/// A dict (or JSON text) as JSON.
fn from_python(value: &Bound<'_, PyAny>) -> PyResult<Value> {
    let text: String = if value.is_instance_of::<PyString>() {
        value.extract()?
    } else {
        value
            .py()
            .import("json")?
            .call_method1("dumps", (value,))?
            .extract()?
    };
    serde_json::from_str(&text)
        .map_err(|e| PyValueError::new_err(format!("the arguments are not JSON: {e}")))
}

fn capability_error(py: Python<'_>, error: &CallError) -> PyErr {
    let raised = CapabilityError::new_err(format!("{}: {}", error.code, error.message));
    let value = raised.value(py);
    let guidance = to_python(py, &error.to_json()["guidance"]);
    let set = value
        .setattr("code", error.code)
        .and_then(|()| value.setattr("message", &error.message))
        .and_then(|()| value.setattr("guidance", guidance?));
    match set {
        Ok(()) => raised,
        Err(error) => error,
    }
}

/// A call's output as Python objects, or its refusal raised.
fn output<'py>(
    py: Python<'py>,
    result: Result<CallResult, CallError>,
) -> PyResult<Bound<'py, PyAny>> {
    match result {
        Ok(result) => to_python(py, &result.output),
        Err(error) => Err(capability_error(py, &error)),
    }
}

fn warn(py: Python<'_>, message: &str) -> PyResult<()> {
    let message = CString::new(message.replace('\0', " ")).expect("no NUL left");
    PyErr::warn(py, &py.get_type::<PyUserWarning>(), &message, 2)
}

/// An I/O error as Python's own: OSError picks the subclass for the error
/// number, e.g. FileNotFoundError, and names the file.
fn os_error(error: std::io::Error, path: PathBuf) -> PyErr {
    match error.raw_os_error() {
        Some(errno) => {
            let text = error.to_string();
            let reason = text
                .strip_suffix(&format!(" (os error {errno})"))
                .unwrap_or(&text);
            let file = path.to_string_lossy().into_owned();
            PyOSError::new_err((errno, reason.to_string(), file))
        }
        None => PyOSError::new_err(format!("{}: {error}", path.display())),
    }
}

fn read_error(error: ScriptReadError) -> PyErr {
    match error {
        ScriptReadError::Io { path, error } => os_error(error, path),
        error => ReadError::new_err(error.to_string()),
    }
}

/// Reads a file, or raises: the format unknown, the file unreadable, no
/// structure imported. Blocks that failed while others were imported are
/// warned about.
fn read_report(
    py: Python<'_>,
    path: &Path,
    format: Option<&str>,
) -> PyResult<(String, FileReport)> {
    let format = crate::input_format(format).map_err(PyValueError::new_err)?;
    let path = path.to_path_buf();
    let report = py
        .detach(move || read_file(&path, format).map(|report| (path, report)))
        .map_err(read_error)?;
    let (path, report) = report;
    let file = path.display().to_string();
    checked(py, &file, report)
}

fn checked(py: Python<'_>, file: &str, report: FileReport) -> PyResult<(String, FileReport)> {
    let failures: Vec<String> = report
        .failed_blocks()
        .map(|block| {
            let (heading, lines) = failure_text(block, MAX_DETAILS);
            format!("{heading}\n    {}", lines.join("\n    "))
        })
        .collect();
    if report.documents.is_empty() {
        let message = if failures.is_empty() {
            format!("{file} holds no structure")
        } else {
            format!("{file}: {}", failures.join("\n"))
        };
        let raised = ReadError::new_err(message);
        let diagnostics = to_python(py, &report.diagnostics_json())?;
        raised.value(py).setattr("diagnostics", diagnostics)?;
        return Err(raised);
    }
    for failure in &failures {
        warn(py, &format!("{file}: {failure}"))?;
    }
    Ok((file.to_string(), report))
}

/// A crystal structure, worked on through the Materials Workbench
/// capability registry.
///
/// Queries return the registry's JSON outputs as dicts. `transform_cell`
/// and `symmetrize` change the structure in place, undoably (`undo`,
/// `redo`); `supercell` returns a new structure, as the application makes
/// one. `call` runs any capability of the registry by name.
#[pyclass(name = "Structure", module = "materials_workbench")]
pub struct Structure {
    document: Document,
}

impl Structure {
    fn new(document: Document) -> Self {
        Self { document }
    }
}

#[pymethods]
impl Structure {
    /// The structure's name: the file's name without extension, with the
    /// data block's when the file holds several.
    #[getter]
    fn name(&self) -> &str {
        self.document.name()
    }

    /// The name of the file the structure came from, directly or as the
    /// source of a supercell; empty for none.
    #[getter]
    fn file(&self) -> &str {
        self.document.file()
    }

    /// Formula and Z, space group and setting, edit mode, cell parameters
    /// (Å, degrees) with those symmetry fixes or links, volume, density,
    /// and the numbers of sites, atoms and symmetry operations
    /// (`structure.summary`).
    fn summary<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        output(py, self.document.summary())
    }

    /// The sites of the asymmetric unit: id, label, fractional position,
    /// multiplicity, Wyckoff position, coordinate form and occupants
    /// (`structure.sites`).
    fn sites<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        output(py, self.document.sites())?.get_item("sites")
    }

    /// What reading the structure's file found: severity, code, line and
    /// message of each finding (`structure.diagnostics`).
    fn diagnostics<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        output(py, self.document.diagnostics())?.get_item("diagnostics")
    }

    /// The symmetry the atoms have and what symmetrizing by it would do;
    /// nothing changes (`structure.detect_symmetry`). `tolerance` is in Å,
    /// 0 for the structure's own (0.01 Å); `standardize` previews the
    /// conventional cell of the standard setting.
    #[pyo3(signature = (tolerance = 0.0, standardize = false))]
    fn detect_symmetry<'py>(
        &mut self,
        py: Python<'py>,
        tolerance: f64,
        standardize: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let document = &mut self.document;
        let result = py.detach(|| document.detect_symmetry(tolerance, standardize));
        output(py, result)
    }

    /// Symmetrizes the structure by the symmetry its atoms have, in place
    /// and undoably (`structure.symmetrize`).
    #[pyo3(signature = (tolerance = 0.0, standardize = false))]
    fn symmetrize<'py>(
        &mut self,
        py: Python<'py>,
        tolerance: f64,
        standardize: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let document = &mut self.document;
        let result = py.detach(|| document.symmetrize(tolerance, standardize));
        output(py, result)
    }

    /// Describes the structure in another cell, in place and undoably; the
    /// atoms stay where they are (`structure.transform_cell`). Give one of
    /// `cell` ("primitive", "conventional" or "niggli"), `setting` (a
    /// Hermann–Mauguin symbol such as "P 1 1 21/a"), or `matrix`, the
    /// change of cell P by rows (column j is the new basis vector j in the
    /// old basis), with `origin` p; their entries are integers, fractions
    /// as text ("1/2"), `fractions.Fraction` or floats such as 0.5.
    #[pyo3(signature = (cell = None, *, setting = None, matrix = None, origin = None))]
    fn transform_cell<'py>(
        &mut self,
        py: Python<'py>,
        cell: Option<&str>,
        setting: Option<&str>,
        matrix: Option<Vec<Vec<Bound<'py, PyAny>>>>,
        origin: Option<Vec<Bound<'py, PyAny>>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let text = |values: Vec<Bound<'py, PyAny>>| -> PyResult<Vec<String>> {
            values.iter().map(fraction).collect()
        };
        let matrix = matrix
            .map(|rows| rows.into_iter().map(text).collect::<PyResult<Vec<_>>>())
            .transpose()?;
        let origin = origin.map(text).transpose()?;
        let new_cell =
            crate::new_cell(cell, setting, matrix, origin).map_err(PyValueError::new_err)?;
        let document = &mut self.document;
        let result = py.detach(|| document.transform_cell(&new_cell));
        output(py, result)
    }

    /// A supercell as a new structure in P1; this one does not change.
    /// `size` is a repeat for every axis (2), three repeats ([2, 2, 1]),
    /// or the matrix M by rows, whose column j is the new vector j in the
    /// old basis ([[1, 1, 0], [-1, 1, 0], [0, 0, 1]]).
    fn supercell(&self, py: Python<'_>, size: &Bound<'_, PyAny>) -> PyResult<Structure> {
        let size = if let Ok(n) = size.extract::<i64>() {
            Size::All(n)
        } else if let Ok(repeats) = size.extract::<Vec<i64>>() {
            Size::Repeats(repeats)
        } else if let Ok(rows) = size.extract::<Vec<Vec<i64>>>() {
            Size::Matrix(rows)
        } else {
            return Err(PyTypeError::new_err(
                "size is an int, three ints, or three rows of three ints",
            ));
        };
        let matrix = crate::supercell_matrix(&size).map_err(PyValueError::new_err)?;
        let document = &self.document;
        match py.detach(|| document.supercell(matrix)) {
            Ok(supercell) => Ok(Structure::new(supercell)),
            Err(error) => Err(capability_error(py, &error)),
        }
    }

    /// The structure as file text: "cif", "poscar" or "xyz", written as
    /// the application's export writes it.
    fn export(&self, py: Python<'_>, format: &str) -> PyResult<String> {
        let format = crate::format(format).map_err(PyValueError::new_err)?;
        let document = &self.document;
        let exported = py
            .detach(|| document.export(format))
            .map_err(|error| capability_error(py, &error))?;
        if !exported.dropped.is_empty() {
            warn(
                py,
                &format!(
                    "{}: fields of the source left out because the structure changed: {}",
                    self.document.name(),
                    exported.dropped.join(", ")
                ),
            )?;
        }
        Ok(exported.text)
    }

    /// Writes the structure to a file, in `format` or the one its name
    /// tells (.cif; .vasp, .poscar, POSCAR*, CONTCAR*; .xyz). A file that
    /// exists is replaced, as with open(path, "w").
    #[pyo3(signature = (path, format = None))]
    fn write(&self, py: Python<'_>, path: PathBuf, format: Option<&str>) -> PyResult<()> {
        let format = crate::output_format(&path, format).map_err(PyValueError::new_err)?;
        let text = self.export(py, format.name())?;
        std::fs::write(&path, text).map_err(|error| os_error(error, path))
    }

    /// Calls a capability of the registry by name on this structure, e.g.
    /// call("structure.neighbors", {"atom": {...}, "cutoff": 3.0}). `args`
    /// is a dict or JSON text; its "structure" is filled in. Returns the
    /// capability's output; `capabilities()` lists them with their
    /// schemas.
    #[pyo3(signature = (capability, args = None))]
    fn call<'py>(
        &mut self,
        py: Python<'py>,
        capability: &str,
        args: Option<&Bound<'py, PyAny>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let input = match args {
            Some(args) if !args.is_none() => from_python(args)?,
            _ => Value::Null,
        };
        let document = &mut self.document;
        let result = py.detach(|| document.call(capability, input));
        output(py, result)
    }

    /// Undoes the last change; False when there was none.
    fn undo(&mut self, py: Python<'_>) -> PyResult<bool> {
        output(py, self.document.undo())?
            .get_item("changed")?
            .extract()
    }

    /// Redoes the last undone change; False when there was none.
    fn redo(&mut self, py: Python<'_>) -> PyResult<bool> {
        output(py, self.document.redo())?
            .get_item("changed")?
            .extract()
    }

    /// An independent copy, with the undo history.
    fn copy(&self) -> Structure {
        Structure::new(self.document.clone())
    }

    fn __copy__(&self) -> Structure {
        self.copy()
    }

    fn __deepcopy__(&self, _memo: &Bound<'_, PyAny>) -> Structure {
        self.copy()
    }

    fn __repr__(&mut self) -> String {
        let name = self.document.name().to_string();
        match self.document.summary() {
            Ok(summary) => {
                let o = &summary.output;
                let group = match &o["space_group"] {
                    Value::Null => "space group not identified".to_string(),
                    sg => format!(
                        "{} (No. {})",
                        sg["symbol"].as_str().unwrap_or_default(),
                        sg["number"]
                    ),
                };
                format!(
                    "<Structure {name:?}: {}, {group}, {} atoms in the cell>",
                    o["formula"].as_str().unwrap_or_default(),
                    o["atoms_in_cell"]
                )
            }
            Err(_) => format!("<Structure {name:?}>"),
        }
    }
}

/// A matrix or origin entry as the text the registry reads.
fn fraction(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let number = if value.is_instance_of::<PyString>() {
        Number::Text(value.extract()?)
    } else if let Ok(n) = value.extract::<i64>() {
        Number::Integer(n)
    } else if let Ok(x) = value.extract::<f64>() {
        Number::Float(x)
    } else {
        // fractions.Fraction and the like say themselves as "1/2".
        Number::Text(value.str()?.to_string())
    };
    Ok(crate::fraction_text(&number))
}

/// Reads the first structure of a file: CIF (.cif) or POSCAR (.vasp,
/// .poscar, POSCAR*, CONTCAR*), or `format` ("cif" or "poscar") whatever
/// the name. Raises ReadError when no structure could be imported, and
/// OSError when the file cannot be read.
#[pyfunction]
#[pyo3(signature = (path, format = None))]
fn read(py: Python<'_>, path: PathBuf, format: Option<&str>) -> PyResult<Structure> {
    let (_, mut report) = read_report(py, &path, format)?;
    Ok(Structure::new(report.documents.remove(0)))
}

/// Reads every structure of a file, e.g. each data block of a CIF.
#[pyfunction]
#[pyo3(signature = (path, format = None))]
fn read_all(py: Python<'_>, path: PathBuf, format: Option<&str>) -> PyResult<Vec<Structure>> {
    let (_, report) = read_report(py, &path, format)?;
    Ok(report.documents.into_iter().map(Structure::new).collect())
}

/// Reads the first structure of file text in `format` ("cif" or
/// "poscar"); it is named after its data block.
#[pyfunction]
fn read_text(py: Python<'_>, text: &str, format: &str) -> PyResult<Structure> {
    let format = crate::input_format(Some(format))
        .map_err(PyValueError::new_err)?
        .expect("a format was given");
    let report = read_bytes(text.as_bytes(), format, None).map_err(read_error)?;
    let (_, mut report) = checked(py, "the text", report)?;
    Ok(Structure::new(report.documents.remove(0)))
}

/// Everything reading a file found, also when nothing could be imported:
/// the format, how many structures were imported, whether it failed, the
/// problems that belong to no block, and each data block with its
/// diagnostics.
#[pyfunction]
#[pyo3(signature = (path, format = None))]
fn diagnostics<'py>(
    py: Python<'py>,
    path: PathBuf,
    format: Option<&str>,
) -> PyResult<Bound<'py, PyAny>> {
    let format = crate::input_format(format).map_err(PyValueError::new_err)?;
    let report = read_file(&path, format).map_err(read_error)?;
    let mut json = report.diagnostics_json();
    json["file"] = Value::String(path.display().to_string());
    to_python(py, &json)
}

/// The capabilities of the registry: name, description, effect,
/// availability and the JSON Schemas of input and output.
#[pyfunction]
fn capabilities(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
    to_python(py, &capabilities_json())
}

/// Crystal structures from Python with Materials Workbench: read CIF and
/// POSCAR files, query them, make supercells, change cells, find and
/// apply symmetry, and write CIF, POSCAR and extended XYZ, through the
/// same capability registry as the application and its command line `mw`.
#[pymodule(name = "materials_workbench")]
mod module {
    use pyo3::prelude::*;

    #[pymodule_export]
    use super::{
        CapabilityError, ReadError, Structure, WorkbenchError, capabilities, diagnostics, read,
        read_all, read_text,
    };

    #[pymodule_init]
    fn init(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add("__version__", env!("CARGO_PKG_VERSION"))
    }
}
