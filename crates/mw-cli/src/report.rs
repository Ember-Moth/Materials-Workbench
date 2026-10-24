//! What a command tells: text on stdout as it goes, or one JSON document at
//! the end; errors on stderr (or in the JSON); the exit status.

use std::io::Write as _;
use std::process::ExitCode;

use mw_automation::CallError;
use serde_json::{Map, Value, json};

/// The exit status for a usage error, as clap uses it.
const USAGE: u8 = 2;

pub struct Report {
    json: bool,
    results: Vec<Value>,
    errors: Vec<Value>,
    /// Top-level fields of the JSON document other than results and errors.
    fields: Map<String, Value>,
    failed: bool,
}

impl Report {
    pub fn new(json: bool) -> Self {
        Self {
            json,
            results: Vec::new(),
            errors: Vec::new(),
            fields: Map::new(),
            failed: false,
        }
    }

    pub fn json(&self) -> bool {
        self.json
    }

    /// A result for the JSON document.
    pub fn result(&mut self, value: Value) {
        self.results.push(value);
    }

    /// A field of the JSON document instead of results and errors.
    pub fn field(&mut self, name: &str, value: Value) {
        self.fields.insert(name.into(), value);
    }

    /// Text for stdout; ignored with `--json`.
    pub fn out(&self, text: &str) {
        if !self.json {
            write_stdout(text);
        }
    }

    /// A message for stderr; ignored with `--json`, where results say it.
    pub fn note(&self, text: &str) {
        if !self.json {
            eprintln!("{text}");
        }
    }

    /// A failure: something the user asked for was not done. Shown on
    /// stderr, or with `--json` among the errors; the exit status is 1.
    pub fn error(&mut self, error: Failure) {
        self.failed = true;
        if self.json {
            self.errors.push(error.to_json());
        } else {
            let mut text = format!("mw: {}: ", error.file);
            if let Some(structure) = &error.structure {
                text.push_str(&format!("{structure}: "));
            }
            text.push_str(&error.message);
            for line in &error.details {
                text.push_str(&format!("\n    {line}"));
            }
            for guidance in &error.guidance {
                text.push_str(&format!("\n    instead: {guidance}"));
            }
            eprintln!("{text}");
        }
    }

    /// Marks the run as failed, for failures the results already show.
    pub fn fail(&mut self) {
        self.failed = true;
    }

    /// Prints the JSON document, if any, and gives the exit status.
    pub fn finish(self) -> ExitCode {
        if self.json {
            let mut document = self.fields;
            if !document.contains_key("capabilities") {
                document.insert("results".into(), Value::Array(self.results));
                document.insert("errors".into(), Value::Array(self.errors));
            }
            let text = serde_json::to_string_pretty(&Value::Object(document))
                .expect("JSON values serialize");
            write_stdout(&format!("{text}\n"));
        }
        if self.failed {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        }
    }
}

/// Stops with a usage error that only shows once the files are read, such
/// as several structures for one `--output`.
pub fn usage(message: &str) -> ! {
    eprintln!("mw: {message}");
    std::process::exit(USAGE.into())
}

/// Writes to stdout. A closed pipe (`mw … | head`) is no error.
fn write_stdout(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush());
}

/// A failed file or structure.
pub struct Failure {
    pub file: String,
    pub structure: Option<String>,
    /// Stable, as the registry's error codes.
    pub code: String,
    pub message: String,
    /// Lines that explain the message, e.g. the errors in the file.
    pub details: Vec<String>,
    /// The diagnostics of a file that could not be imported, as
    /// `structure.diagnostics` gives them.
    pub diagnostics: Vec<Value>,
    pub guidance: Vec<Value>,
}

impl Failure {
    pub fn new(file: &str, code: &str, message: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            structure: None,
            code: code.into(),
            message: message.into(),
            details: Vec::new(),
            diagnostics: Vec::new(),
            guidance: Vec::new(),
        }
    }

    /// A capability's error for one structure.
    pub fn call(file: &str, structure: &str, error: &CallError) -> Self {
        let guidance = match error.to_json()["guidance"].take() {
            Value::Array(guidance) => guidance,
            _ => Vec::new(),
        };
        Self {
            structure: Some(structure.into()),
            guidance,
            ..Self::new(file, error.code, &error.message)
        }
    }

    pub fn structure(mut self, structure: &str) -> Self {
        self.structure = Some(structure.into());
        self
    }

    fn to_json(&self) -> Value {
        json!({
            "file": self.file,
            "structure": self.structure,
            "code": self.code,
            "message": self.message,
            "diagnostics": self.diagnostics,
            "guidance": self.guidance,
        })
    }
}
