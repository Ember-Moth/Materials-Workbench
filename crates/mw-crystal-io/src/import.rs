//! CIF semantics: one data block to one [`Structure`] (design §9.1).
//!
//! Reads the cell, the symmetry operations, the space group name and the
//! atom sites with their ADPs and disorder, under the current or the legacy
//! (`_symmetry_*`) field names. Anything that breaks a hard constraint is an
//! error and the block yields no structure; everything the importer decides
//! on the user's behalf is reported.
//!
//! A structure is then normalized (see `normalize`): atoms snap onto
//! special positions, rows at one position merge into mixed occupancy.
//! Finally the space group type is identified from the operations and
//! checked against the name the file gives and against the cell.
//!
//! Mock: unrecognized fields are not kept for export yet (M5).

use std::collections::{HashMap, HashSet, VecDeque};
use std::f64::consts::PI;

use mw_crystal::{
    Adp, CellParams, Disorder, Lattice, Measured, Occupant, Site, SiteId, SpaceGroup,
    SpaceGroupType, Species, Structure, SymOp, Symmetry, SymmetryError, UNKNOWN_ELEMENT, element,
};

use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use crate::normalize::{self, OCCUPANCY_TOLERANCE};
use crate::number;
use crate::syntax::{Block, Item, Value};

const SYMOP_TAGS: [&str; 2] = [
    "_space_group_symop_operation_xyz",
    "_symmetry_equiv_pos_as_xyz",
];
const HM_TAGS: [&str; 2] = [
    "_space_group_name_h-m_alt",
    "_symmetry_space_group_name_h-m",
];
const IT_NUMBER_TAGS: [&str; 2] = ["_space_group_it_number", "_symmetry_int_tables_number"];
const HALL_TAGS: [&str; 2] = ["_space_group_name_hall", "_symmetry_space_group_name_hall"];
const LENGTH_TAGS: [&str; 3] = ["_cell_length_a", "_cell_length_b", "_cell_length_c"];
const ANGLE_TAGS: [&str; 3] = ["_cell_angle_alpha", "_cell_angle_beta", "_cell_angle_gamma"];
const ADP_U_TAGS: [&str; 6] = [
    "_atom_site_aniso_u_11",
    "_atom_site_aniso_u_22",
    "_atom_site_aniso_u_33",
    "_atom_site_aniso_u_12",
    "_atom_site_aniso_u_13",
    "_atom_site_aniso_u_23",
];
const ADP_B_TAGS: [&str; 6] = [
    "_atom_site_aniso_b_11",
    "_atom_site_aniso_b_22",
    "_atom_site_aniso_b_33",
    "_atom_site_aniso_b_12",
    "_atom_site_aniso_b_13",
    "_atom_site_aniso_b_23",
];

/// Differences between the cell and the nearest cell the operations
/// preserve below this (Å or degrees) are rounding, not inconsistency: in
/// the COD, a third of the cells that differ at all differ by less, such as
/// b = 32.246 written for a = 32.2456(13).
const CELL_DEVIATION: f64 = 1e-3;

/// U = B / 8π².
fn b_to_u(b: Measured) -> Measured {
    let factor = 8.0 * PI * PI;
    match b.su() {
        Some(su) => Measured::with_su(b.value() / factor, su / factor),
        None => Measured::exact(b.value() / factor),
    }
}

/// A category's values as rows, from a loop or from single pairs.
struct Table<'a> {
    columns: HashMap<&'a str, usize>,
    rows: Vec<Vec<&'a Value>>,
    line: u32,
}

impl<'a> Table<'a> {
    fn get(&self, row: &[&'a Value], tag: &str) -> Option<&'a Value> {
        self.columns.get(tag).map(|&column| row[column])
    }

    fn has(&self, tag: &str) -> bool {
        self.columns.contains_key(tag)
    }
}

/// Lookup of a block's values by tag.
struct Fields<'a> {
    block: &'a Block,
}

impl<'a> Fields<'a> {
    fn has(&self, tag: &str) -> bool {
        self.values(tag).is_some()
    }

    /// The tag is present with some value other than `?` and `.`.
    fn has_value(&self, tag: &str) -> bool {
        self.values(tag)
            .is_some_and(|values| values.iter().any(|v| !v.is_missing()))
    }

    /// The values of a tag: one for a pair, a column for a loop.
    fn values(&self, tag: &str) -> Option<Vec<&'a Value>> {
        self.block.items.iter().find_map(|item| match item {
            Item::Pair { tag: t, value } => (t.name == tag).then(|| vec![value]),
            Item::Loop(table) => table
                .column(tag)
                .map(|column| table.rows().map(|row| &row[column]).collect()),
        })
    }

    fn value(&self, tag: &str) -> Option<&'a Value> {
        self.values(tag)?.into_iter().next()
    }

    /// The first of several names for one field that is present. Reports
    /// another name that is also present with a value that is not the
    /// `same`.
    fn alias(
        &self,
        tags: &[&'static str],
        same: fn(&[&Value], &[&Value]) -> bool,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<&'static str> {
        let present: Vec<(&'static str, Vec<&Value>)> = tags
            .iter()
            .filter_map(|tag| Some((*tag, self.values(tag)?)))
            .collect();
        let (used, used_values) = present.first()?;
        for (ignored, values) in &present[1..] {
            if !same(values, used_values) {
                diagnostics.push(Diagnostic::new(
                    DiagnosticKind::AliasConflict {
                        used: (*used).to_string(),
                        ignored: (*ignored).to_string(),
                    },
                    values.first().map(|v| v.line),
                ));
            }
        }
        Some(used)
    }

    /// The rows of the category that `anchor` belongs to. In a loop, that
    /// loop; as pairs, every pair whose tag starts with `prefix` but not
    /// with `exclude`.
    fn table(&self, anchor: &str, prefix: &str, exclude: Option<&str>) -> Option<Table<'a>> {
        for item in &self.block.items {
            match item {
                Item::Loop(table) if table.column(anchor).is_some() => {
                    return Some(Table {
                        columns: table
                            .tags
                            .iter()
                            .enumerate()
                            .map(|(i, tag)| (tag.name.as_str(), i))
                            .collect(),
                        rows: table.rows().map(|row| row.iter().collect()).collect(),
                        line: table.line,
                    });
                }
                Item::Pair { tag, .. } if tag.name == anchor => {
                    let mut columns = HashMap::new();
                    let mut row = Vec::new();
                    for item in &self.block.items {
                        if let Item::Pair { tag, value } = item
                            && tag.name.starts_with(prefix)
                            && !exclude.is_some_and(|e| tag.name.starts_with(e))
                        {
                            columns.insert(tag.name.as_str(), row.len());
                            row.push(value);
                        }
                    }
                    return Some(Table {
                        columns,
                        rows: vec![row],
                        line: tag.line,
                    });
                }
                _ => {}
            }
        }
        None
    }
}

/// Two values of one field under different names agree: the same text up
/// to case and spaces.
fn same_text(a: &[&Value], b: &[&Value]) -> bool {
    let normalize = |values: &[&Value]| {
        values
            .iter()
            .map(|v| v.text.split_whitespace().collect::<String>().to_lowercase())
            .collect::<Vec<_>>()
    };
    normalize(a) == normalize(b)
}

/// Two operation lists agree when they hold the same operations, in any
/// order and spelling.
fn same_operations(a: &[&Value], b: &[&Value]) -> bool {
    let set = |values: &[&Value]| {
        values
            .iter()
            .map(|v| SymOp::parse(&v.text).ok())
            .collect::<Option<HashSet<_>>>()
    };
    match (set(a), set(b)) {
        (Some(a), Some(b)) => a == b,
        _ => same_text(a, b),
    }
}

/// Two Hermann–Mauguin symbols agree when they name the same setting, such
/// as `P 21/n` and `P 1 21/n 1`; `F d -3 m` and `F d -3 m :1` do not.
fn same_setting(a: &[&Value], b: &[&Value]) -> bool {
    let setting = |values: &[&Value]| Symmetry::from_hm_symbol(values.first()?.text.trim());
    match (setting(a), setting(b)) {
        (Some(a), Some(b)) => a == b,
        _ => same_text(a, b),
    }
}

/// A number field: `Ok(None)` for `?` and `.`, an error diagnostic for text
/// that is no number.
fn number(
    value: &Value,
    tag: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Option<Measured>, ()> {
    if value.is_missing() {
        return Ok(None);
    }
    match number::parse(&value.text) {
        Some(measured) => Ok(Some(measured)),
        None => {
            diagnostics.push(Diagnostic::at(
                DiagnosticKind::InvalidNumber {
                    tag: tag.to_string(),
                    text: value.text.clone(),
                },
                value.line,
            ));
            Err(())
        }
    }
}

/// A displacement parameter: `None` for `?` and `.`, and for text that is
/// no number, which is only a warning. ADPs are kept for export, not used,
/// so a typo in one should not cost the structure.
fn adp_number(
    value: &Value,
    tag: &str,
    label: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Measured> {
    if value.is_missing() {
        return None;
    }
    let parsed = number::parse(&value.text);
    if parsed.is_none() {
        diagnostics.push(Diagnostic::at(
            DiagnosticKind::InvalidAdp {
                label: label.to_string(),
                tag: tag.to_string(),
                text: value.text.clone(),
            },
            value.line,
        ));
    }
    parsed
}

/// Interprets one data block. The structure is `None` when any diagnostic,
/// including a syntax error in the block, is an error.
pub(crate) fn interpret(block: &Block) -> (Option<Structure>, Vec<Diagnostic>) {
    let mut diagnostics = block.diagnostics.clone();
    let fields = Fields { block };
    // Placeholder entries give the tags with nothing but `?`.
    let has_cell = LENGTH_TAGS.iter().any(|tag| fields.has_value(tag));
    let has_atoms = [
        "_atom_site_label",
        "_atom_site_fract_x",
        "_atom_site_cartn_x",
    ]
    .iter()
    .any(|tag| fields.has_value(tag));
    if !has_cell && !has_atoms {
        diagnostics.push(Diagnostic::at(DiagnosticKind::NoStructure, block.line));
        return (None, diagnostics);
    }

    let lattice = read_cell(&fields, block.line, &mut diagnostics);
    let (space_group, name_line) = read_space_group(&fields, &mut diagnostics);
    let symmetry = read_symmetry(&fields, &space_group, block.line, &mut diagnostics);
    let sites = read_sites(&fields, block.line, &mut diagnostics);

    let structure = match (lattice, symmetry, sites) {
        (Some(lattice), Some(symmetry), Some(sites))
            if !diagnostics.iter().any(|d| d.severity() == Severity::Error) =>
        {
            let mut structure =
                Structure::new(lattice, symmetry, sites.sites).with_space_group(space_group);
            let lines: HashMap<SiteId, u32> =
                sites.rows.iter().map(|(id, row)| (*id, row.line)).collect();
            normalize::snap_sites(&mut structure, &lines, &mut diagnostics);
            normalize::merge_sites(&mut structure, &lines, &mut diagnostics);
            check_geometry(&structure, &sites.rows, &mut diagnostics);
            let cell_line = fields.value(LENGTH_TAGS[0]).map(|v| v.line);
            check_symmetry(&structure, name_line, cell_line, &mut diagnostics);
            Some(structure)
        }
        _ => None,
    };
    // Stable: diagnostics on one line keep the order they were found in.
    diagnostics.sort_by_key(|d| d.line().unwrap_or(u32::MAX));
    (structure, diagnostics)
}

fn read_cell(
    fields: &Fields,
    block_line: u32,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Lattice> {
    let mut lengths = [Measured::exact(0.0); 3];
    let mut complete = true;
    for (length, tag) in lengths.iter_mut().zip(LENGTH_TAGS) {
        let value = fields.value(tag);
        match value.map(|v| number(v, tag, diagnostics)) {
            Some(Ok(Some(measured))) => *length = measured,
            Some(Err(())) => complete = false,
            Some(Ok(None)) | None => {
                complete = false;
                diagnostics.push(Diagnostic::at(
                    DiagnosticKind::MissingCellParameter {
                        tag: tag.to_string(),
                    },
                    value.map_or(block_line, |v| v.line),
                ));
            }
        }
    }
    let mut angles = [Measured::exact(90.0); 3];
    for (angle, tag) in angles.iter_mut().zip(ANGLE_TAGS) {
        let value = fields.value(tag);
        match value.map(|v| number(v, tag, diagnostics)) {
            Some(Ok(Some(measured))) => *angle = measured,
            Some(Err(())) => complete = false,
            // The core dictionary's default for cell angles is 90°.
            Some(Ok(None)) | None => diagnostics.push(Diagnostic::at(
                DiagnosticKind::AngleDefaulted {
                    tag: tag.to_string(),
                },
                value.map_or(block_line, |v| v.line),
            )),
        }
    }
    if !complete {
        return None;
    }
    let [a, b, c] = lengths;
    let [alpha, beta, gamma] = angles;
    let params = CellParams {
        a,
        b,
        c,
        alpha,
        beta,
        gamma,
    };
    if !params.is_valid() {
        let line = fields.value(LENGTH_TAGS[0]).map_or(block_line, |v| v.line);
        diagnostics.push(Diagnostic::at(DiagnosticKind::InvalidCell, line));
        return None;
    }
    Some(Lattice::new(params))
}

/// The space group as the file names it, and the line of the name.
fn read_space_group(
    fields: &Fields,
    diagnostics: &mut Vec<Diagnostic>,
) -> (SpaceGroup, Option<u32>) {
    let hm_value = fields
        .alias(&HM_TAGS, same_setting, diagnostics)
        .and_then(|tag| fields.value(tag))
        .filter(|v| !v.is_missing() && !v.text.trim().is_empty());
    let hm_symbol = hm_value.map(|v| v.text.trim().to_string());
    let it_value = fields
        .alias(&IT_NUMBER_TAGS, same_text, diagnostics)
        .and_then(|tag| fields.value(tag))
        .filter(|v| !v.is_missing());
    let line = hm_value.or(it_value).map(|v| v.line);
    let it_number = it_value.and_then(|v| match v.text.parse::<u16>() {
        Ok(n) if (1..=230).contains(&n) => Some(n),
        _ => {
            diagnostics.push(Diagnostic::at(
                DiagnosticKind::InvalidItNumber {
                    text: v.text.clone(),
                },
                v.line,
            ));
            None
        }
    });
    (SpaceGroup::new(hm_symbol, it_number), line)
}

/// Operations from the space group name when the file lists none, in the
/// order of design §4.4: Hall symbol, Hermann–Mauguin symbol, IT number.
fn symmetry_from_name(
    fields: &Fields,
    space_group: &SpaceGroup,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<(Symmetry, String)> {
    let hall = fields
        .alias(&HALL_TAGS, same_text, diagnostics)
        .and_then(|tag| fields.value(tag))
        .filter(|v| !v.is_missing())
        .map(|v| v.text.trim().to_string());
    if let Some(hall) = hall
        && let Some(symmetry) = Symmetry::from_hall_symbol(&hall)
    {
        return Some((symmetry, hall));
    }
    if let Some(symbol) = &space_group.hm_symbol
        && let Some(symmetry) = Symmetry::from_hm_symbol(symbol)
    {
        return Some((symmetry, symbol.clone()));
    }
    let number = space_group.it_number?;
    Some((Symmetry::from_it_number(number)?, number.to_string()))
}

fn read_symmetry(
    fields: &Fields,
    space_group: &SpaceGroup,
    block_line: u32,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<Symmetry> {
    let Some(tag) = fields.alias(&SYMOP_TAGS, same_operations, diagnostics) else {
        if let Some((symmetry, name)) = symmetry_from_name(fields, space_group, diagnostics) {
            // P1 needs no operations, so naming it is no fallback.
            if !symmetry.is_p1() {
                diagnostics.push(Diagnostic::at(
                    DiagnosticKind::SymopsFromName { name },
                    block_line,
                ));
            }
            return Some(symmetry);
        }
        let name = space_group
            .hm_symbol
            .clone()
            .or_else(|| space_group.it_number.map(|n| n.to_string()));
        diagnostics.push(Diagnostic::at(
            DiagnosticKind::MissingSymops { name },
            block_line,
        ));
        return None;
    };
    let values = fields.values(tag).unwrap_or_default();
    let mut operations: Vec<(SymOp, &Value)> = Vec::new();
    let mut seen = HashSet::new();
    let mut valid = true;
    for value in values {
        match SymOp::parse(&value.text) {
            Ok(op) if !seen.insert(op) => diagnostics.push(Diagnostic::at(
                DiagnosticKind::DuplicateSymop {
                    text: value.text.clone(),
                },
                value.line,
            )),
            Ok(op) => operations.push((op, value)),
            Err(error) => {
                valid = false;
                diagnostics.push(Diagnostic::at(
                    DiagnosticKind::InvalidSymop {
                        text: value.text.clone(),
                        error,
                    },
                    value.line,
                ));
            }
        }
    }
    if !valid {
        return None;
    }
    let first_line = operations.first().map_or(block_line, |(_, v)| v.line);
    match Symmetry::new(operations.iter().map(|(op, _)| *op).collect()) {
        Ok(symmetry) => Some(symmetry),
        Err(error) => {
            let involved: Vec<usize> = match error {
                SymmetryError::MissingIdentity => Vec::new(),
                SymmetryError::Singular(i) => vec![i],
                SymmetryError::Duplicate(a, b) | SymmetryError::NotClosed(a, b) => vec![a, b],
            };
            let line = involved
                .first()
                .map_or(first_line, |&i| operations[i].1.line);
            diagnostics.push(Diagnostic::at(
                DiagnosticKind::InvalidSymmetry {
                    error,
                    operations: involved
                        .iter()
                        .map(|&i| operations[i].1.text.clone())
                        .collect(),
                },
                line,
            ));
            None
        }
    }
}

/// The element and oxidation state a type symbol such as `Fe3+` or `O2-`
/// names, or `None` if it names no element.
fn parse_type_symbol(text: &str) -> Option<(String, Option<i8>)> {
    let letters: String = text.chars().take_while(char::is_ascii_alphabetic).collect();
    let rest = &text[letters.len()..];
    let element = element_from_letters(&letters)?;
    // Charge as `3+`, `+3`, `+` or `2-`; anything else is not a charge.
    let oxidation = {
        let (digits, sign) = if let Some(digits) = rest.strip_suffix(['+', '-']) {
            (digits, rest.chars().last())
        } else if let Some(digits) = rest.strip_prefix(['+', '-']) {
            (digits, rest.chars().next())
        } else {
            ("", None)
        };
        match (sign, digits) {
            (Some(sign), digits) if digits.chars().all(|c| c.is_ascii_digit()) => {
                let magnitude: i8 = if digits.is_empty() {
                    1
                } else {
                    digits.parse().ok()?
                };
                Some(if sign == '-' { -magnitude } else { magnitude })
            }
            _ => None,
        }
    };
    Some((element, oxidation))
}

/// The element a symbol or label starts with: a two-letter symbol when its
/// second letter is lowercase (`Ca1` is calcium), otherwise a one-letter
/// symbol (`CA1` is carbon), otherwise a capitalized two-letter one (`FE`).
/// `D` (deuterium) is hydrogen.
fn element_from_letters(letters: &str) -> Option<String> {
    let mut chars = letters.chars();
    let first = chars.next()?.to_ascii_uppercase();
    let second = chars.next();
    if let Some(second) = second.filter(char::is_ascii_lowercase) {
        let symbol = format!("{first}{second}");
        if element::is_element(&symbol) {
            return Some(symbol);
        }
    }
    if first == 'D' {
        return Some("H".into());
    }
    let symbol = first.to_string();
    if element::is_element(&symbol) {
        return Some(symbol);
    }
    let symbol = format!("{first}{}", second?.to_ascii_lowercase());
    element::is_element(&symbol).then_some(symbol)
}

/// Sites read from the atom site loop, and per site its row.
struct SiteRows {
    sites: Vec<Site>,
    rows: HashMap<SiteId, Row>,
}

/// What a row of the atom site loop states about its site, for the checks
/// after normalization.
struct Row {
    line: u32,
    /// `_atom_site_symmetry_multiplicity`: the number of equivalent atoms in
    /// the cell, except in SHELXL-97 files, which give the site-symmetry
    /// order under this name (see `check_geometry`).
    multiplicity: Option<u32>,
    /// `_atom_site_site_symmetry_order`: how many operations leave the site
    /// in place.
    order: Option<u32>,
}

/// Anisotropic ADPs by label, each with whether it was given as B and its
/// line. A label the file repeats has one entry per row, in file order.
type AnisoRows = HashMap<String, VecDeque<(Adp, bool, u32)>>;

fn read_aniso(fields: &Fields, diagnostics: &mut Vec<Diagnostic>) -> AnisoRows {
    let mut adps = AnisoRows::new();
    let Some(table) = fields.table("_atom_site_aniso_label", "_atom_site_aniso_", None) else {
        return adps;
    };
    let (tags, is_b) = if table.has(ADP_U_TAGS[0]) {
        (ADP_U_TAGS, false)
    } else if table.has(ADP_B_TAGS[0]) {
        (ADP_B_TAGS, true)
    } else {
        return adps;
    };
    for row in &table.rows {
        let Some(label) = table.get(row, "_atom_site_aniso_label") else {
            continue;
        };
        let mut components = [Measured::exact(0.0); 6];
        let mut complete = true;
        for (component, tag) in components.iter_mut().zip(tags) {
            match table
                .get(row, tag)
                .and_then(|v| adp_number(v, tag, &label.text, diagnostics))
            {
                Some(m) => *component = if is_b { b_to_u(m) } else { m },
                // An unknown component leaves the atom isotropic.
                None => complete = false,
            }
        }
        if complete {
            adps.entry(label.text.clone()).or_default().push_back((
                Adp::Anisotropic(components),
                is_b,
                label.line,
            ));
        }
    }
    adps
}

fn read_sites(
    fields: &Fields,
    block_line: u32,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SiteRows> {
    let Some(table) = fields.table(
        "_atom_site_fract_x",
        "_atom_site_",
        Some("_atom_site_aniso_"),
    ) else {
        let kind = if fields.has("_atom_site_cartn_x") {
            DiagnosticKind::CartesianOnly
        } else {
            DiagnosticKind::MissingAtomSites
        };
        diagnostics.push(Diagnostic::at(kind, block_line));
        return None;
    };
    let required = [
        "_atom_site_label",
        "_atom_site_fract_x",
        "_atom_site_fract_y",
        "_atom_site_fract_z",
    ];
    let missing: Vec<_> = required.iter().filter(|tag| !table.has(tag)).collect();
    for tag in &missing {
        diagnostics.push(Diagnostic::at(
            DiagnosticKind::MissingAtomTag {
                tag: tag.to_string(),
            },
            table.line,
        ));
    }
    if !missing.is_empty() {
        return None;
    }

    let mut aniso = read_aniso(fields, diagnostics);
    let mut sites = Vec::new();
    let mut rows = HashMap::new();
    let mut labels = HashSet::new();
    let file_labels: HashSet<&str> = table
        .rows
        .iter()
        .filter_map(|row| table.get(row, "_atom_site_label"))
        .map(|v| v.text.as_str())
        .collect();
    let mut from_label = 0;
    let mut occupancy_assumed = 0;
    let mut b_converted = 0;
    let mut valid = true;

    for row in &table.rows {
        // A placeholder row in an entry without coordinates.
        if row.iter().all(|v| v.is_missing()) {
            continue;
        }
        let label_value = table.get(row, "_atom_site_label").expect("required");
        let line = label_value.line;
        let mut label = label_value.text.clone();
        // The ADP the file gives this row: a repeated label takes the
        // anisotropic rows of its label in turn.
        let adp = aniso.get_mut(&label).and_then(VecDeque::pop_front);
        // Dummy rows can leave every other value out, the label included.
        if table
            .get(row, "_atom_site_calc_flag")
            .is_some_and(|v| v.text.eq_ignore_ascii_case("dum"))
        {
            diagnostics.push(Diagnostic::at(
                DiagnosticKind::DummyAtomSkipped { label },
                line,
            ));
            continue;
        }
        if label_value.is_missing() {
            valid = false;
            diagnostics.push(Diagnostic::at(
                DiagnosticKind::MissingAtomValue {
                    label,
                    tag: "_atom_site_label".into(),
                },
                line,
            ));
            continue;
        }
        if labels.contains(&label) {
            // Labels name sites, so a repeat is renamed rather than lost:
            // SHELXL-97 files reuse hydrogen labels such as H1B.
            let renamed = (2..)
                .map(|n| format!("{label}_{n}"))
                .find(|l| !labels.contains(l) && !file_labels.contains(l.as_str()))
                .expect("a free label");
            diagnostics.push(Diagnostic::at(
                DiagnosticKind::DuplicateLabel {
                    label,
                    renamed: renamed.clone(),
                },
                line,
            ));
            label = renamed;
        }

        let mut position = [0.0; 3];
        for (coordinate, tag) in position.iter_mut().zip(&required[1..]) {
            let value = table.get(row, tag).expect("required");
            match number(value, tag, diagnostics) {
                Ok(Some(m)) => *coordinate = m.value(),
                Ok(None) => {
                    valid = false;
                    diagnostics.push(Diagnostic::at(
                        DiagnosticKind::MissingAtomValue {
                            label: label.clone(),
                            tag: tag.to_string(),
                        },
                        value.line,
                    ));
                }
                Err(()) => valid = false,
            }
        }

        let species = match table
            .get(row, "_atom_site_type_symbol")
            .filter(|v| !v.is_missing())
        {
            Some(symbol) => match parse_type_symbol(&symbol.text) {
                Some((element, oxidation)) => Species::new(&element, oxidation, &symbol.text),
                None => {
                    diagnostics.push(Diagnostic::at(
                        DiagnosticKind::UnknownElement {
                            label: label.clone(),
                            symbol: symbol.text.clone(),
                        },
                        symbol.line,
                    ));
                    Species::new(UNKNOWN_ELEMENT, None, &symbol.text)
                }
            },
            None => {
                let letters: String = label
                    .chars()
                    .take_while(char::is_ascii_alphabetic)
                    .collect();
                match element_from_letters(&letters) {
                    Some(element) => {
                        from_label += 1;
                        Species::new(&element, None, &element)
                    }
                    None => {
                        diagnostics.push(Diagnostic::at(
                            DiagnosticKind::UnknownElement {
                                label: label.clone(),
                                symbol: label.clone(),
                            },
                            line,
                        ));
                        Species::new(UNKNOWN_ELEMENT, None, &label)
                    }
                }
            }
        };

        let occupancy = match table.get(row, "_atom_site_occupancy") {
            None => Measured::exact(1.0),
            Some(value) => match number(value, "_atom_site_occupancy", diagnostics) {
                // An absent atom, such as a disorder component refined to
                // 0.00.
                Ok(Some(m)) if m.value() <= 0.0 => {
                    diagnostics.push(Diagnostic::at(
                        DiagnosticKind::InvalidOccupancy {
                            label: label.clone(),
                            value: m.value(),
                        },
                        value.line,
                    ));
                    continue;
                }
                Ok(Some(m)) => {
                    if m.value() > 1.0 + OCCUPANCY_TOLERANCE {
                        diagnostics.push(Diagnostic::at(
                            DiagnosticKind::OccupancyAboveOne {
                                label: label.clone(),
                                value: m.value(),
                            },
                            value.line,
                        ));
                    }
                    m
                }
                Ok(None) => {
                    occupancy_assumed += 1;
                    Measured::exact(1.0)
                }
                Err(()) => {
                    valid = false;
                    Measured::exact(1.0)
                }
            },
        };

        let mut occupant = Occupant::new(&label, species, occupancy);
        let isotropic = |tag: &str, diagnostics: &mut Vec<Diagnostic>| {
            table
                .get(row, tag)
                .and_then(|v| adp_number(v, tag, &label, diagnostics))
        };
        if let Some((adp, is_b, _)) = adp {
            b_converted += usize::from(is_b);
            occupant = occupant.with_adp(adp);
        } else if let Some(u) = isotropic("_atom_site_u_iso_or_equiv", diagnostics) {
            occupant = occupant.with_adp(Adp::Isotropic(u));
        } else if let Some(b) = isotropic("_atom_site_b_iso_or_equiv", diagnostics) {
            b_converted += 1;
            occupant = occupant.with_adp(Adp::Isotropic(b_to_u(b)));
        }

        labels.insert(label.clone());
        let id = SiteId(sites.len() as u32 + 1);
        let mut site = Site::new(id, position, vec![occupant]);
        let text = |tag: &str| {
            table
                .get(row, tag)
                .filter(|v| !v.is_missing())
                .map(|v| v.text.clone())
        };
        if let Some(group) = text("_atom_site_disorder_group") {
            site = site.with_disorder(Disorder::new(text("_atom_site_disorder_assembly"), group));
        }
        let integer = |tag: &str| text(tag).and_then(|t| t.parse().ok()).filter(|n| *n > 0);
        rows.insert(
            id,
            Row {
                line,
                multiplicity: integer("_atom_site_symmetry_multiplicity"),
                order: integer("_atom_site_site_symmetry_order"),
            },
        );
        sites.push(site);
    }

    for (label, rows) in aniso {
        for (_, _, line) in rows {
            diagnostics.push(Diagnostic::at(
                DiagnosticKind::AnisoUnknownLabel {
                    label: label.clone(),
                },
                line,
            ));
        }
    }
    if valid && sites.is_empty() {
        diagnostics.push(Diagnostic::at(DiagnosticKind::MissingAtomSites, table.line));
        return None;
    }
    for (count, kind) in [
        (
            from_label,
            DiagnosticKind::ElementFromLabel { count: from_label },
        ),
        (
            occupancy_assumed,
            DiagnosticKind::OccupancyAssumed {
                count: occupancy_assumed,
            },
        ),
        (
            b_converted,
            DiagnosticKind::BisoConverted { count: b_converted },
        ),
    ] {
        if count > 0 {
            diagnostics.push(Diagnostic::at(kind, table.line));
        }
    }
    valid.then_some(SiteRows { sites, rows })
}

/// Reports what the expansion of the normalized structure reveals: atoms
/// that likely belong on a special position but are too far from it to
/// snap, multiplicities that disagree with the file, and sites that come
/// too close. Sites at one position were dealt with by `merge_sites`.
///
/// The file states a site's multiplicity m directly, or through its
/// site-symmetry order k = |operations| / m. SHELXL-97 writes k under the
/// multiplicity tag (1 on a general position); a block is read in the
/// convention most of its values fit.
fn check_geometry(
    structure: &Structure,
    rows: &HashMap<SiteId, Row>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let orbits = mw_crystal::orbits(structure);
    let tolerances = structure.tolerances;
    // A fully occupied, ordered site: anything close to it is a conflict
    // rather than an alternative position.
    let is_full = |id: SiteId| {
        structure.site(id).is_some_and(|site| {
            site.disorder.is_none()
                && site
                    .occupants
                    .iter()
                    .map(|o| o.occupancy.value())
                    .sum::<f64>()
                    >= 1.0 - OCCUPANCY_TOLERANCE
        })
    };
    let label = |id: SiteId| {
        structure
            .site(id)
            .map_or_else(String::new, |s| s.label().to_string())
    };
    let line = |id: SiteId| rows.get(&id).map(|row| row.line);
    let operations = structure.symmetry.operations().len();
    let fits_multiplicity = |stated: u32, atoms: usize| stated as usize == atoms;
    let fits_order = |stated: u32, atoms: usize| stated as usize * atoms == operations;
    let fitting = |fits: &dyn Fn(u32, usize) -> bool| {
        orbits
            .iter()
            .filter_map(|o| Some((rows.get(&o.site)?.multiplicity?, o.positions.len())))
            .filter(|&(stated, atoms)| fits(stated, atoms))
            .count()
    };
    let multiplicity_is_order = fitting(&fits_order) > fitting(&fits_multiplicity);

    for orbit in &orbits {
        let close = orbit
            .nearest_image
            .filter(|d| *d < tolerances.short_contact && is_full(orbit.site));
        if let Some(distance) = close {
            diagnostics.push(Diagnostic::new(
                DiagnosticKind::CloseImages {
                    label: label(orbit.site),
                    distance,
                },
                line(orbit.site),
            ));
            continue;
        }
        let Some(row) = rows.get(&orbit.site) else {
            continue;
        };
        let atoms = orbit.positions.len();
        // The multiplicity the file states or implies, if it disagrees.
        let file = match (row.multiplicity, row.order) {
            (Some(m), _) if !multiplicity_is_order => (!fits_multiplicity(m, atoms)).then_some(m),
            (Some(k), _) | (None, Some(k)) => {
                (!fits_order(k, atoms)).then_some(operations as u32 / k)
            }
            (None, None) => None,
        };
        if let Some(file) = file {
            diagnostics.push(Diagnostic::new(
                DiagnosticKind::MultiplicityMismatch {
                    label: label(orbit.site),
                    file,
                    expanded: atoms,
                },
                line(orbit.site),
            ));
        }
    }

    for contact in mw_crystal::close_contacts(structure, &orbits, tolerances.short_contact) {
        if contact.distance < tolerances.position || !(is_full(contact.a) && is_full(contact.b)) {
            continue;
        }
        diagnostics.push(Diagnostic::new(
            DiagnosticKind::ShortContact {
                a: label(contact.a),
                b: label(contact.b),
                distance: contact.distance,
            },
            line(contact.b),
        ));
    }
}

/// Identifies the space group type of the operations and reports a name in
/// the file that disagrees with it (the operations win, design §4.4) and
/// cell parameters the operations do not preserve (design §7.3).
fn check_symmetry(
    structure: &Structure,
    name_line: Option<u32>,
    cell_line: Option<u32>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let params = structure.lattice.params();
    let (length, angle) = mw_crystal::cell_deviation(&structure.symmetry, params);
    if length > CELL_DEVIATION || angle > CELL_DEVIATION {
        diagnostics.push(Diagnostic::new(
            DiagnosticKind::CellInconsistent { length, angle },
            cell_line,
        ));
    }
    let Some(identified) = SpaceGroupType::identify(&structure.symmetry, params) else {
        diagnostics.push(Diagnostic::new(
            DiagnosticKind::UnidentifiedSpaceGroup,
            name_line,
        ));
        return;
    };
    let named = &structure.space_group;
    let named_number = named.it_number.or_else(|| {
        named
            .hm_symbol
            .as_deref()
            .and_then(mw_crystal::it_number_of)
    });
    if named_number.is_some_and(|n| n != identified.number) {
        diagnostics.push(Diagnostic::new(
            DiagnosticKind::SpaceGroupMismatch {
                file_symbol: named.hm_symbol.clone(),
                file_number: named.it_number,
                number: identified.number,
                symbol: identified.hm_symbol.to_string(),
            },
            name_line,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_symbols() {
        let parse = |t: &str| parse_type_symbol(t);
        assert_eq!(parse("Fe3+"), Some(("Fe".into(), Some(3))));
        assert_eq!(parse("O2-"), Some(("O".into(), Some(-2))));
        assert_eq!(parse("Na+"), Some(("Na".into(), Some(1))));
        assert_eq!(parse("Ti+4"), Some(("Ti".into(), Some(4))));
        assert_eq!(parse("Sr"), Some(("Sr".into(), None)));
        assert_eq!(parse("D"), Some(("H".into(), None)));
        assert_eq!(parse("Q"), None);
    }

    #[test]
    fn elements_from_labels() {
        let from = |label: &str| {
            let letters: String = label
                .chars()
                .take_while(char::is_ascii_alphabetic)
                .collect();
            element_from_letters(&letters)
        };
        assert_eq!(from("Ti1").as_deref(), Some("Ti"));
        assert_eq!(from("Ca2").as_deref(), Some("Ca"));
        assert_eq!(from("CA2").as_deref(), Some("C"));
        assert_eq!(from("OW1").as_deref(), Some("O"));
        assert_eq!(from("FE1").as_deref(), Some("F"));
        assert_eq!(from("ZN1").as_deref(), Some("Zn"));
        assert_eq!(from("O1A").as_deref(), Some("O"));
        assert_eq!(from("Q1"), None);
    }

    /// A block with the given space group lines, cell and atom rows
    /// (label, symbol, x, y, z, occupancy and optional disorder group).
    fn block(header: &str, cell: [f64; 6], atoms: &[&str]) -> (Option<Structure>, Vec<String>) {
        let mut text = format!("data_test\n{header}\n");
        for (tag, value) in [
            "_cell_length_a",
            "_cell_length_b",
            "_cell_length_c",
            "_cell_angle_alpha",
            "_cell_angle_beta",
            "_cell_angle_gamma",
        ]
        .iter()
        .zip(cell)
        {
            text.push_str(&format!("{tag} {value}\n"));
        }
        text.push_str(
            "loop_\n_atom_site_label\n_atom_site_type_symbol\n_atom_site_fract_x\n\
             _atom_site_fract_y\n_atom_site_fract_z\n_atom_site_occupancy\n\
             _atom_site_disorder_group\n",
        );
        for atom in atoms {
            text.push_str(atom);
            text.push('\n');
        }
        let report = crate::cif::read_str(&text);
        let block = &report.blocks[0];
        let codes = block
            .diagnostics
            .iter()
            .map(|d| d.code().to_string())
            .collect();
        (block.structure.clone(), codes)
    }

    const CUBIC: [f64; 6] = [4.0, 4.0, 4.0, 90.0, 90.0, 90.0];

    #[test]
    fn operations_from_the_space_group_number() {
        let (structure, codes) = block("_space_group_IT_number 221", CUBIC, &["Sr1 Sr 0 0 0 1 ."]);
        assert_eq!(codes, ["symmetry.operations_from_name"]);
        assert_eq!(structure.unwrap().symmetry.operations().len(), 48);
    }

    #[test]
    fn unknown_names_without_operations_fail() {
        let (structure, codes) = block(
            "_space_group_name_H-M_alt 'Q 9'",
            CUBIC,
            &["Sr1 Sr 0 0 0 1 ."],
        );
        assert!(structure.is_none());
        assert_eq!(codes, ["symmetry.missing_operations"]);
    }

    #[test]
    fn a_name_that_contradicts_the_operations_is_reported() {
        let header =
            "_space_group_name_H-M_alt 'P 4/m m m'\n_space_group_symop_operation_xyz 'x,y,z'";
        let (structure, codes) = block(header, CUBIC, &["Sr1 Sr 0 0 0 1 ."]);
        assert!(structure.is_some());
        assert_eq!(codes, ["symmetry.name_mismatch"]);
    }

    #[test]
    fn a_cell_the_operations_do_not_preserve_is_reported() {
        let header = "_space_group_IT_number 221";
        let cell = [3.905, 3.91, 3.905, 90.0, 90.0, 90.0];
        let (_, codes) = block(header, cell, &["Sr1 Sr 0 0 0 1 ."]);
        // Sorted by line: the block header comes first.
        assert_eq!(
            codes,
            [
                "symmetry.operations_from_name",
                "cell.inconsistent_with_symmetry"
            ]
        );
        // A difference in the last printed digit is rounding.
        let cell = [3.9052, 3.905, 3.905, 90.0, 90.0, 90.0];
        let (_, codes) = block(header, cell, &["Sr1 Sr 0 0 0 1 ."]);
        assert_eq!(codes, ["symmetry.operations_from_name"]);
    }

    #[test]
    fn rows_at_one_position() {
        let header = "_space_group_IT_number 221";
        // Different species, both full: a conflict, kept.
        let (structure, codes) = block(header, CUBIC, &["Sr1 Sr 0 0 0 1 .", "Ba1 Ba 0 0 0 1 ."]);
        assert_eq!(structure.unwrap().sites.len(), 2);
        assert_eq!(codes[1], "atom.overlapping_sites");
        // Alternative positions in different disorder groups: kept apart.
        let (structure, codes) =
            block(header, CUBIC, &["Sr1 Sr 0 0 0 0.5 1", "Ba1 Ba 0 0 0 0.5 2"]);
        assert_eq!(structure.unwrap().sites.len(), 2);
        assert_eq!(codes[1], "atom.shared_position");
        // Merged, but the occupancies add up to 1.4: reported on Sr1's row,
        // the merge on Ba1's.
        let (structure, codes) =
            block(header, CUBIC, &["Sr1 Sr 0 0 0 0.7 .", "Ba1 Ba 0 0 0 0.7 ."]);
        assert_eq!(structure.unwrap().sites.len(), 1);
        assert_eq!(
            codes[1..],
            ["atom.occupancy_sum_above_one", "atom.merged_occupancy"]
        );
    }

    #[test]
    fn b_converts_to_u() {
        let u = b_to_u(Measured::with_su(0.79, 0.08));
        assert!((u.value() - 0.010_005).abs() < 1e-6);
        assert!((u.su().unwrap() - 0.001_013).abs() < 1e-6);
    }
}
