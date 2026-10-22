//! The JSON Schema subset tool definitions are written in, and validation of
//! tool arguments against it (design §6.2.6).
//!
//! The subset is what both Anthropic strict tool use and OpenAI Structured
//! Outputs accept, narrowed further so that this module can validate every
//! keyword itself:
//!
//! - every object has `properties`, lists all of them in `required`, and sets
//!   `additionalProperties: false`; optional values use a `null` union such
//!   as `"type": ["number", "null"]`;
//! - only the keywords in [`ALLOWED_KEYWORDS`];
//! - the root is a plain object; `anyOf` and `$ref` only below it, and
//!   references are local (`#/$defs/...`) and not recursive.
//!
//! Ranges, lengths, patterns and formats go in `description` and are checked
//! by the tool's handler.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::{Map, Value, json};

use crate::{ToolCall, ToolDefinition, ToolResult};

/// Keywords allowed anywhere in a tool schema.
pub const ALLOWED_KEYWORDS: &[&str] = &[
    "type",
    "description",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "anyOf",
    "$ref",
    "$defs",
    "minItems",
];

const TYPES: &[&str] = &[
    "object", "array", "string", "number", "integer", "boolean", "null",
];

/// OpenAI's nesting limit.
const MAX_DEPTH: usize = 10;
/// Anthropic: at most 20 strict tools per request.
pub const MAX_STRICT_TOOLS: usize = 20;
/// Anthropic: at most 16 parameters with union types across all strict tools.
pub const MAX_UNION_PARAMETERS: usize = 16;
/// OpenAI: at most 1000 enum values per schema; applied per request.
pub const MAX_ENUM_VALUES: usize = 1000;

/// A schema construct outside the subset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaIssue {
    /// Tool name, empty for a schema checked on its own.
    pub tool: String,
    /// JSON pointer into the schema; `None` for issues with the tool name or
    /// the request as a whole.
    pub path: Option<String>,
    pub message: String,
}

impl fmt::Display for SchemaIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.tool.is_empty() {
            write!(f, "{}: ", self.tool)?;
        }
        match self.path.as_deref() {
            Some("") => write!(f, "/: {}", self.message),
            Some(path) => write!(f, "{path}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// Counts that the per-request limits apply to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SchemaStats {
    /// Nodes with `anyOf` or a type list.
    pub union_parameters: usize,
    pub enum_values: usize,
}

/// Checks one tool's input schema against the subset.
pub fn check_schema(schema: &Value) -> Result<SchemaStats, Vec<SchemaIssue>> {
    let mut checker = Checker::default();
    checker.check_root(schema);
    if checker.issues.is_empty() {
        Ok(checker.stats)
    } else {
        Err(checker.issues)
    }
}

/// Checks a request's tool set: names and schemas, and with `strict` also the
/// limits strict mode puts on a whole request.
pub fn check_tools(tools: &[ToolDefinition], strict: bool) -> Result<(), Vec<SchemaIssue>> {
    let mut issues = Vec::new();
    let mut names = BTreeSet::new();
    let mut totals = SchemaStats::default();
    let issue = |tool: &str, message: String| SchemaIssue {
        tool: tool.to_owned(),
        path: None,
        message,
    };
    for tool in tools {
        if !valid_tool_name(&tool.name) {
            issues.push(issue(
                &tool.name,
                "tool names must match ^[a-zA-Z0-9_-]{1,64}$".into(),
            ));
        }
        if !names.insert(tool.name.as_str()) {
            issues.push(issue(&tool.name, "duplicate tool name".into()));
        }
        match check_schema(&tool.input_schema) {
            Ok(stats) => {
                totals.union_parameters += stats.union_parameters;
                totals.enum_values += stats.enum_values;
            }
            Err(found) => issues.extend(found.into_iter().map(|found| SchemaIssue {
                tool: tool.name.clone(),
                ..found
            })),
        }
    }
    if !strict {
        return if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        };
    }
    if tools.len() > MAX_STRICT_TOOLS {
        issues.push(issue(
            "",
            format!(
                "{} tools; at most {MAX_STRICT_TOOLS} strict tools per request",
                tools.len()
            ),
        ));
    }
    if totals.union_parameters > MAX_UNION_PARAMETERS {
        issues.push(issue(
            "",
            format!(
                "{} parameters with union types; at most {MAX_UNION_PARAMETERS} per request",
                totals.union_parameters
            ),
        ));
    }
    if totals.enum_values > MAX_ENUM_VALUES {
        issues.push(issue(
            "",
            format!(
                "{} enum values; at most {MAX_ENUM_VALUES} per request",
                totals.enum_values
            ),
        ));
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

fn valid_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[derive(Default)]
struct Checker {
    issues: Vec<SchemaIssue>,
    stats: SchemaStats,
    defs: BTreeSet<String>,
    /// References made from each definition (`""` for the root schema).
    refs: BTreeMap<String, BTreeSet<String>>,
}

impl Checker {
    fn issue(&mut self, path: &str, message: impl Into<String>) {
        self.issues.push(SchemaIssue {
            tool: String::new(),
            path: Some(path.to_owned()),
            message: message.into(),
        });
    }

    fn check_root(&mut self, schema: &Value) {
        let Some(root) = schema.as_object() else {
            self.issue("", "the schema must be a JSON object");
            return;
        };
        if root.get("type") != Some(&json!("object")) {
            self.issue("", "the root must have \"type\": \"object\"");
        }
        if root.contains_key("anyOf") || root.contains_key("$ref") {
            self.issue("", "the root cannot use anyOf or $ref");
        }
        let defs = root.get("$defs");
        if let Some(defs) = defs {
            match defs.as_object() {
                Some(defs) => self.defs = defs.keys().cloned().collect(),
                None => self.issue("/$defs", "$defs must be an object"),
            }
        }
        self.check_node(schema, "", "", 1);
        if let Some(defs) = defs.and_then(Value::as_object) {
            for (name, def) in defs {
                let path = format!("/$defs/{}", escape(name));
                self.check_node(def, &path, name, 1);
            }
        }
        self.check_recursion();
    }

    fn check_node(&mut self, node: &Value, path: &str, def: &str, depth: usize) {
        let Some(map) = node.as_object() else {
            self.issue(path, "a schema must be a JSON object");
            return;
        };
        for key in map.keys() {
            if key == "$defs" && path.is_empty() {
                continue;
            }
            if !ALLOWED_KEYWORDS.contains(&key.as_str()) || key == "$defs" {
                self.issue(
                    path,
                    format!("keyword \"{key}\" is not in the strict subset"),
                );
            }
        }
        if let Some(description) = map.get("description")
            && !description.is_string()
        {
            self.issue(path, "description must be a string");
        }

        if let Some(reference) = map.get("$ref") {
            self.check_ref(map, reference, path, def);
            return;
        }
        if let Some(branches) = map.get("anyOf") {
            self.check_any_of(map, branches, path, def, depth);
            return;
        }
        let Some(types) = self.types(map.get("type"), path) else {
            return;
        };
        self.check_by_type(map, &types, path, def, depth);
    }

    fn check_ref(&mut self, map: &Map<String, Value>, reference: &Value, path: &str, def: &str) {
        if map.keys().any(|key| key != "$ref" && key != "description") {
            self.issue(path, "$ref can only be combined with description");
        }
        let target = reference
            .as_str()
            .and_then(|reference| reference.strip_prefix("#/$defs/"))
            .map(unescape);
        match target {
            Some(target) if self.defs.contains(&target) => {
                self.refs.entry(def.to_owned()).or_default().insert(target);
            }
            _ => self.issue(path, "$ref must point to an entry of the root $defs"),
        }
    }

    fn check_any_of(
        &mut self,
        map: &Map<String, Value>,
        branches: &Value,
        path: &str,
        def: &str,
        depth: usize,
    ) {
        self.stats.union_parameters += 1;
        if map.keys().any(|key| key != "anyOf" && key != "description") {
            self.issue(path, "anyOf can only be combined with description");
        }
        match branches.as_array() {
            Some(branches) if !branches.is_empty() => {
                for (index, branch) in branches.iter().enumerate() {
                    self.check_node(branch, &format!("{path}/anyOf/{index}"), def, depth);
                }
            }
            _ => self.issue(path, "anyOf must be a non-empty array"),
        }
    }

    /// The declared types: one name, or one name plus `"null"`.
    fn types(&mut self, value: Option<&Value>, path: &str) -> Option<Vec<String>> {
        let types: Vec<String> = match value {
            None => {
                self.issue(path, "every schema needs \"type\", \"anyOf\" or \"$ref\"");
                return None;
            }
            Some(Value::String(name)) => vec![name.clone()],
            Some(Value::Array(names)) => {
                self.stats.union_parameters += 1;
                let names: Option<Vec<String>> = names
                    .iter()
                    .map(|name| name.as_str().map(str::to_owned))
                    .collect();
                let Some(names) = names else {
                    self.issue(path, "type names must be strings");
                    return None;
                };
                let non_null = names.iter().filter(|name| *name != "null").count();
                if names.len() != 2 || non_null != 1 {
                    self.issue(path, "a type list must be one type plus \"null\"");
                    return None;
                }
                names
            }
            Some(_) => {
                self.issue(path, "type must be a string or a list of strings");
                return None;
            }
        };
        if let Some(unknown) = types.iter().find(|name| !TYPES.contains(&name.as_str())) {
            self.issue(path, format!("unknown type \"{unknown}\""));
            return None;
        }
        Some(types)
    }

    fn check_by_type(
        &mut self,
        map: &Map<String, Value>,
        types: &[String],
        path: &str,
        def: &str,
        depth: usize,
    ) {
        let is = |name: &str| types.iter().any(|t| t == name);

        if is("object") {
            self.check_object(map, path, def, depth);
        } else {
            for key in ["properties", "required", "additionalProperties"] {
                if map.contains_key(key) {
                    self.issue(path, format!("{key} is only allowed on objects"));
                }
            }
        }

        if is("array") {
            match map.get("items") {
                Some(items) if items.is_object() => {
                    self.check_node(items, &format!("{path}/items"), def, depth + 1);
                }
                _ => self.issue(path, "arrays need a single schema in items"),
            }
            if let Some(min) = map.get("minItems")
                && min != &json!(0)
                && min != &json!(1)
            {
                self.issue(path, "minItems can only be 0 or 1");
            }
        } else {
            for key in ["items", "minItems"] {
                if map.contains_key(key) {
                    self.issue(path, format!("{key} is only allowed on arrays"));
                }
            }
        }

        if let Some(values) = map.get("enum") {
            match values.as_array() {
                Some(values) if !values.is_empty() => {
                    self.stats.enum_values += values.len();
                    for value in values {
                        if value.is_array() || value.is_object() {
                            self.issue(
                                path,
                                "enum values must be strings, numbers, booleans or null",
                            );
                        } else if !types.iter().any(|t| matches_type(value, t)) {
                            self.issue(path, format!("enum value {value} does not match the type"));
                        }
                    }
                }
                _ => self.issue(path, "enum must be a non-empty array"),
            }
        }
    }

    fn check_object(&mut self, map: &Map<String, Value>, path: &str, def: &str, depth: usize) {
        if depth > MAX_DEPTH {
            self.issue(
                path,
                format!("objects can be nested at most {MAX_DEPTH} levels deep"),
            );
        }
        if map.get("additionalProperties") != Some(&Value::Bool(false)) {
            self.issue(path, "objects must set \"additionalProperties\": false");
        }
        let Some(properties) = map.get("properties").and_then(Value::as_object) else {
            self.issue(path, "objects must have properties (it may be empty)");
            return;
        };
        let required: Option<BTreeSet<&str>> = map
            .get("required")
            .and_then(Value::as_array)
            .and_then(|list| list.iter().map(Value::as_str).collect());
        let declared: BTreeSet<&str> = properties.keys().map(String::as_str).collect();
        match required {
            Some(required) if required == declared => {
                let listed = map["required"].as_array().map_or(0, Vec::len);
                if listed != required.len() {
                    self.issue(path, "required lists a property twice");
                }
            }
            Some(_) => self.issue(
                path,
                "required must list every property; make optional values nullable",
            ),
            None => self.issue(path, "objects must have required, a list of property names"),
        }
        for (name, property) in properties {
            let child = format!("{path}/properties/{}", escape(name));
            self.check_node(property, &child, def, depth + 1);
        }
    }

    fn check_recursion(&mut self) {
        // Depth-first search for a cycle through the definitions.
        fn visit(
            name: &str,
            refs: &BTreeMap<String, BTreeSet<String>>,
            stack: &mut Vec<String>,
            done: &mut BTreeSet<String>,
        ) -> Option<String> {
            if stack.iter().any(|entry| entry == name) {
                return Some(name.to_owned());
            }
            if done.contains(name) {
                return None;
            }
            stack.push(name.to_owned());
            for next in refs.get(name).into_iter().flatten() {
                if let Some(cycle) = visit(next, refs, stack, done) {
                    return Some(cycle);
                }
            }
            stack.pop();
            done.insert(name.to_owned());
            None
        }
        let mut done = BTreeSet::new();
        let starts: Vec<String> = self.refs.keys().cloned().collect();
        for start in starts {
            if let Some(cycle) = visit(&start, &self.refs, &mut Vec::new(), &mut done) {
                self.issue(
                    &format!("/$defs/{}", escape(&cycle)),
                    "recursive schemas are not allowed",
                );
                return;
            }
        }
    }
}

/// JSON pointer escaping of one path segment.
fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn unescape(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

fn matches_type(value: &Value, name: &str) -> bool {
    match name {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64()
                || value.is_u64()
                || value
                    .as_f64()
                    .is_some_and(|x| x.is_finite() && x.fract() == 0.0)
        }
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Why a tool call's arguments were rejected.
#[derive(Clone, Debug, PartialEq)]
pub enum ArgumentError {
    /// The arguments are not valid JSON (possible with non-strict services,
    /// or Anthropic's unbuffered tool input streaming).
    InvalidJson { message: String },
    /// Valid JSON that does not fit the tool's schema.
    Invalid(Vec<Violation>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// JSON pointer into the arguments; empty for the whole value.
    pub path: String,
    pub message: String,
}

/// At most this many violations are reported back to the model.
const MAX_VIOLATIONS: usize = 20;
/// Malformed input is echoed back to the model up to this many characters.
const MAX_ECHO: usize = 2000;

impl ArgumentError {
    /// The error as a tool result for the model, so it can correct the call
    /// (design §4 "错误可恢复").
    pub fn to_tool_result(&self, call: &ToolCall) -> ToolResult {
        ToolResult::error(&call.id, self.to_model_text(&call.arguments_raw))
    }

    /// Compact JSON, in English like all text for the model.
    pub fn to_model_text(&self, raw_arguments: &str) -> String {
        let body = match self {
            Self::InvalidJson { message } => {
                let mut received: String = raw_arguments.chars().take(MAX_ECHO).collect();
                if received.len() < raw_arguments.len() {
                    received.push('…');
                }
                json!({
                    "error": "invalid_json",
                    "message": format!("The arguments are not valid JSON: {message}"),
                    "received": received,
                    "hint": "Call the tool again with a single valid JSON object.",
                })
            }
            Self::Invalid(violations) => json!({
                "error": "invalid_arguments",
                "violations": violations
                    .iter()
                    .map(|v| json!({ "path": v.path, "message": v.message }))
                    .collect::<Vec<_>>(),
                "hint": "Fix these arguments and call the tool again.",
            }),
        };
        body.to_string()
    }
}

impl fmt::Display for ArgumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson { message } => write!(f, "arguments are not valid JSON: {message}"),
            Self::Invalid(violations) => {
                f.write_str("arguments do not match the schema: ")?;
                for (index, violation) in violations.iter().enumerate() {
                    if index > 0 {
                        f.write_str("; ")?;
                    }
                    let path = if violation.path.is_empty() {
                        "/"
                    } else {
                        &violation.path
                    };
                    write!(f, "{path} {}", violation.message)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ArgumentError {}

/// Parses `call`'s arguments and validates them against `schema`, which
/// should have passed [`check_schema`]. Returns the parsed arguments.
pub fn validate_arguments(schema: &Value, call: &ToolCall) -> Result<Value, ArgumentError> {
    let value = if call.arguments_raw.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&call.arguments_raw).map_err(|error| ArgumentError::InvalidJson {
            message: error.to_string(),
        })?
    };
    let defs = schema.get("$defs").and_then(Value::as_object);
    let mut violations = Vec::new();
    validate(schema, &value, "", defs, 0, &mut violations);
    if violations.is_empty() {
        Ok(value)
    } else {
        violations.truncate(MAX_VIOLATIONS);
        Err(ArgumentError::Invalid(violations))
    }
}

fn validate(
    schema: &Value,
    value: &Value,
    path: &str,
    defs: Option<&Map<String, Value>>,
    depth: usize,
    out: &mut Vec<Violation>,
) {
    let violation = |message: String| Violation {
        path: path.to_owned(),
        message,
    };
    // Recursive schemas are rejected by the checker; this only guards
    // against looping on one that was never checked.
    if depth > 64 || out.len() >= MAX_VIOLATIONS {
        return;
    }
    let Some(map) = schema.as_object() else {
        return;
    };

    if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
        let target = reference
            .strip_prefix("#/$defs/")
            .map(unescape)
            .and_then(|name| defs?.get(&name));
        match target {
            Some(target) => validate(target, value, path, defs, depth + 1, out),
            None => out.push(violation(format!(
                "unresolvable schema reference {reference}"
            ))),
        }
        return;
    }

    if let Some(branches) = map.get("anyOf").and_then(Value::as_array) {
        let matches = branches.iter().any(|branch| {
            let mut scratch = Vec::new();
            validate(branch, value, path, defs, depth + 1, &mut scratch);
            scratch.is_empty()
        });
        if !matches {
            out.push(violation(format!(
                "matches none of the {} allowed shapes",
                branches.len()
            )));
        }
        return;
    }

    let types: Vec<&str> = match map.get("type") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !types.is_empty() && !types.iter().any(|name| matches_type(value, name)) {
        out.push(violation(format!(
            "expected {}, got {}",
            types.join(" or "),
            type_name(value)
        )));
        return;
    }

    if let Some(allowed) = map.get("enum").and_then(Value::as_array)
        && !allowed.iter().any(|candidate| json_equal(candidate, value))
    {
        let listed: Vec<String> = allowed.iter().map(Value::to_string).collect();
        out.push(violation(format!("must be one of {}", listed.join(", "))));
        return;
    }

    match value {
        Value::Object(object) => validate_object(map, object, path, defs, depth, out),
        Value::Array(items) => {
            if let Some(min) = map.get("minItems").and_then(Value::as_u64)
                && (items.len() as u64) < min
            {
                out.push(violation(format!("needs at least {min} item(s)")));
            }
            if let Some(item_schema) = map.get("items") {
                for (index, item) in items.iter().enumerate() {
                    validate(
                        item_schema,
                        item,
                        &format!("{path}/{index}"),
                        defs,
                        depth + 1,
                        out,
                    );
                }
            }
        }
        _ => {}
    }
}

fn validate_object(
    schema: &Map<String, Value>,
    object: &Map<String, Value>,
    path: &str,
    defs: Option<&Map<String, Value>>,
    depth: usize,
    out: &mut Vec<Violation>,
) {
    let properties = schema.get("properties").and_then(Value::as_object);
    for name in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !object.contains_key(name) {
            out.push(Violation {
                path: format!("{path}/{}", escape(name)),
                message: "is required but missing".into(),
            });
        }
    }
    let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));
    for (name, value) in object {
        let child = format!("{path}/{}", escape(name));
        match properties.and_then(|properties| properties.get(name)) {
            Some(property) => validate(property, value, &child, defs, depth + 1, out),
            None if closed => out.push(Violation {
                path: child,
                message: "is not a known property".into(),
            }),
            None => {}
        }
    }
}

/// JSON equality with numbers compared by value, so `1` equals `1.0`.
fn json_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supercell() -> Value {
        json!({
            "type": "object",
            "description": "Build a supercell.",
            "properties": {
                "structure_id": { "type": "string" },
                "repeats": {
                    "type": "array",
                    "description": "Repetitions along a, b, c; each 1 to 10.",
                    "items": { "type": "integer" },
                    "minItems": 1
                },
                "label": { "type": ["string", "null"] },
                "mode": { "type": "string", "enum": ["p1", "symmetric"] },
                "origin": { "$ref": "#/$defs/vector" },
                "target": {
                    "anyOf": [
                        { "type": "string" },
                        {
                            "type": "object",
                            "properties": { "site": { "type": "integer" } },
                            "required": ["site"],
                            "additionalProperties": false
                        }
                    ]
                }
            },
            "required": ["structure_id", "repeats", "label", "mode", "origin", "target"],
            "additionalProperties": false,
            "$defs": {
                "vector": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "number" },
                        "y": { "type": "number" },
                        "z": { "type": "number" }
                    },
                    "required": ["x", "y", "z"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn messages(schema: &Value) -> Vec<String> {
        check_schema(schema)
            .unwrap_err()
            .into_iter()
            .map(|issue| issue.to_string())
            .collect()
    }

    #[test]
    fn accepts_the_subset() {
        let stats = check_schema(&supercell()).unwrap();
        assert_eq!(stats.union_parameters, 2);
        assert_eq!(stats.enum_values, 2);
        check_schema(&json!({
            "type": "object", "properties": {}, "required": [], "additionalProperties": false
        }))
        .unwrap();
    }

    #[test]
    fn rejects_open_objects_and_optional_fields() {
        let found = messages(&json!({
            "type": "object",
            "properties": {
                "a": { "type": "number" },
                "b": { "type": "object", "properties": {}, "required": [] }
            },
            "required": ["a"]
        }));
        assert_eq!(
            found,
            [
                "/: objects must set \"additionalProperties\": false",
                "/: required must list every property; make optional values nullable",
                "/properties/b: objects must set \"additionalProperties\": false",
            ]
        );
    }

    #[test]
    fn rejects_keywords_outside_the_allowlist() {
        let found = messages(&json!({
            "type": "object",
            "title": "x",
            "properties": {
                "n": { "type": "number", "minimum": 0 },
                "s": { "type": "string", "format": "uuid", "pattern": "^a" },
                "list": { "type": "array", "items": { "type": "string" }, "minItems": 2, "maxItems": 3 },
                "k": { "type": "string", "const": "x", "default": "x" }
            },
            "required": ["n", "s", "list", "k"],
            "additionalProperties": false
        }));
        for keyword in [
            "title", "minimum", "format", "pattern", "maxItems", "const", "default",
        ] {
            assert!(
                found.iter().any(|m| m.contains(&format!("\"{keyword}\""))),
                "{keyword} not reported in {found:?}"
            );
        }
        assert!(
            found
                .iter()
                .any(|m| m.contains("minItems can only be 0 or 1"))
        );
    }

    #[test]
    fn rejects_root_unions_bad_type_lists_and_recursion() {
        let found = messages(&json!({ "anyOf": [{ "type": "object" }] }));
        assert!(found[0].contains("the root must have \"type\": \"object\""));

        let found = messages(&json!({
            "type": "object",
            "properties": { "a": { "type": ["string", "number"] } },
            "required": ["a"],
            "additionalProperties": false
        }));
        assert_eq!(
            found,
            ["/properties/a: a type list must be one type plus \"null\""]
        );

        let found = messages(&json!({
            "type": "object",
            "properties": { "tree": { "$ref": "#/$defs/node" } },
            "required": ["tree"],
            "additionalProperties": false,
            "$defs": {
                "node": {
                    "type": "object",
                    "properties": { "children": { "type": "array", "items": { "$ref": "#/$defs/node" } } },
                    "required": ["children"],
                    "additionalProperties": false
                }
            }
        }));
        assert_eq!(found, ["/$defs/node: recursive schemas are not allowed"]);

        let found = messages(&json!({
            "type": "object",
            "properties": { "a": { "$ref": "#/$defs/missing" } },
            "required": ["a"],
            "additionalProperties": false
        }));
        assert_eq!(
            found,
            ["/properties/a: $ref must point to an entry of the root $defs"]
        );
    }

    #[test]
    fn checks_names_and_request_limits() {
        let empty = json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false });
        let tool = |name: &str| ToolDefinition::new(name, "", empty.clone());
        let found = check_tools(
            &[tool("structure.make_supercell"), tool("a"), tool("a")],
            false,
        )
        .unwrap_err();
        assert_eq!(
            found[0].message,
            "tool names must match ^[a-zA-Z0-9_-]{1,64}$"
        );
        assert_eq!(found[1].message, "duplicate tool name");

        let many: Vec<ToolDefinition> = (0..21).map(|i| tool(&format!("t{i}"))).collect();
        assert!(
            check_tools(&many, true).unwrap_err()[0]
                .message
                .contains("at most 20")
        );
        check_tools(&many, false).unwrap();

        let nullable = json!({
            "type": "object",
            "properties": (0..17).map(|i| (format!("p{i}"), json!({ "type": ["string", "null"] }))).collect::<Map<_, _>>(),
            "required": (0..17).map(|i| format!("p{i}")).collect::<Vec<_>>(),
            "additionalProperties": false
        });
        let found = check_tools(&[ToolDefinition::new("t", "", nullable)], true).unwrap_err();
        assert!(found[0].message.contains("17 parameters with union types"));
    }

    fn call(arguments: &str) -> ToolCall {
        ToolCall::new("call_1", "make_supercell", arguments)
    }

    #[test]
    fn valid_arguments_pass_and_are_returned() {
        let arguments = json!({
            "structure_id": "s1", "repeats": [2, 2, 2.0], "label": null, "mode": "p1",
            "origin": { "x": 0, "y": 0.5, "z": 0 }, "target": { "site": 3 }
        });
        let parsed = validate_arguments(&supercell(), &call(&arguments.to_string())).unwrap();
        assert_eq!(parsed, arguments);
    }

    #[test]
    fn schema_violations_are_reported_with_paths() {
        let arguments = json!({
            "structure_id": 7, "repeats": [], "mode": "P1",
            "origin": { "x": 0, "y": "half", "z": 0, "w": 1 }, "target": { "site": "3" }, "extra": true
        });
        let Err(ArgumentError::Invalid(violations)) =
            validate_arguments(&supercell(), &call(&arguments.to_string()))
        else {
            panic!("expected violations");
        };
        let mut found: Vec<(&str, &str)> = violations
            .iter()
            .map(|v| (v.path.as_str(), v.message.as_str()))
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                ("/extra", "is not a known property"),
                ("/label", "is required but missing"),
                ("/mode", "must be one of \"p1\", \"symmetric\""),
                ("/origin/w", "is not a known property"),
                ("/origin/y", "expected number, got string"),
                ("/repeats", "needs at least 1 item(s)"),
                ("/structure_id", "expected string, got number"),
                ("/target", "matches none of the 2 allowed shapes"),
            ]
        );
    }

    #[test]
    fn invalid_json_becomes_an_error_result_for_the_model() {
        let call = call(r#"{"structure_id": "s1", "repeats": [2, 2"#);
        let error = validate_arguments(&supercell(), &call).unwrap_err();
        assert!(matches!(error, ArgumentError::InvalidJson { .. }));
        let result = error.to_tool_result(&call);
        assert!(result.is_error);
        assert_eq!(result.call_id, "call_1");
        let body: Value = serde_json::from_str(&result.joined_text()).unwrap();
        assert_eq!(body["error"], "invalid_json");
        assert_eq!(body["received"], call.arguments_raw);
    }

    #[test]
    fn empty_arguments_count_as_an_empty_object() {
        let schema = json!({ "type": "object", "properties": {}, "required": [], "additionalProperties": false });
        assert_eq!(validate_arguments(&schema, &call("")).unwrap(), json!({}));
        assert!(validate_arguments(&schema, &call("[]")).is_err());
    }
}
