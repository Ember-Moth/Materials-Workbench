//! CIF export (design §9.2): the operations written out, the space group
//! named when it is identified, the atom sites with their occupancies,
//! ADPs and standard uncertainties, and the derived fields regenerated
//! from the structure. Fields of the source file that the structure does
//! not hold are written back as the design's round-trip strategy says
//! (§6): descriptive ones always, unrecognized ones while the structure is
//! unchanged.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use mw_crystal::{
    Adp, CoordinateConstraint, CrystalSystem, Measured, Setting, SpaceGroupType, Structure,
    SymmetryAnalysis,
};

use crate::kept::{CifFields, Class, Column, Field, Text};

/// What [`write`] adds to the structure.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriteOptions<'a> {
    block_name: &'a str,
    kept: Option<&'a CifFields>,
    structure_changed: bool,
}

impl<'a> WriteOptions<'a> {
    /// The data block name, e.g. `rutile` for `data_rutile`. Characters CIF
    /// 1.1 does not allow there become `_`; with none left, the block is
    /// `data_structure`.
    pub fn new(block_name: &'a str) -> Self {
        Self {
            block_name,
            kept: None,
            structure_changed: false,
        }
    }

    /// The fields of the block the structure was read from. Unrecognized
    /// ones are left out when the structure changed since, and reported
    /// in [`Written::dropped`].
    pub fn kept(mut self, fields: &'a CifFields, structure_changed: bool) -> Self {
        self.kept = Some(fields);
        self.structure_changed = structure_changed;
        self
    }
}

/// A written CIF file.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Written {
    pub text: String,
    /// Tags of the source file left out because the structure changed
    /// since it was read, in file order.
    pub dropped: Vec<String>,
}

/// Writes one data block for the structure.
pub fn write(structure: &Structure, options: &WriteOptions) -> Written {
    let orbits = mw_crystal::orbits(structure);
    let analysis = SymmetryAnalysis::compute(structure, &orbits);
    let space_group = analysis.space_group.as_ref();

    let mut cif = Cif::default();
    cif.out
        .push_str("#\\#CIF_1.1\n# Written by Materials Workbench\n");
    let _ = writeln!(cif.out, "data_{}", block_name(options.block_name));

    // The writer's own tags, which a kept field must not repeat.
    let mut sections = Cif::default();
    write_cell(&mut sections, structure, &orbits);
    write_space_group(&mut sections, structure, space_group);
    let kept = options.kept;
    let changed = options.structure_changed;
    write_sites(&mut sections, structure, &analysis, &orbits, kept, changed);

    let mut dropped = Vec::new();
    if let Some(kept) = kept {
        for (class, field) in &kept.fields {
            let tags: Vec<&str> = field.tags().collect();
            if *class == Class::Unrecognized && changed {
                dropped.extend(tags.iter().map(|tag| tag.to_string()));
                continue;
            }
            if tags
                .iter()
                .any(|tag| sections.tags.contains(&tag.to_ascii_lowercase()))
            {
                continue;
            }
            match field {
                Field::Pair { tag, value } => cif.pair(tag, kept_value(value)),
                Field::Loop { tags, values } => {
                    let rows = values
                        .chunks(tags.len().max(1))
                        .map(|row| row.iter().map(kept_value).collect())
                        .collect();
                    cif.blank();
                    cif.table(tags, rows);
                    cif.blank();
                }
            }
        }
        if changed {
            for column in kept.site_columns.iter().chain(&kept.aniso_columns) {
                dropped.push(column.tag.clone());
            }
        }
    }
    cif.blank();
    cif.out.push_str(&sections.out);
    Written {
        text: cif.out,
        dropped,
    }
}

/// CIF text being written, and the tags in it.
#[derive(Default)]
struct Cif {
    out: String,
    tags: HashSet<String>,
}

/// Tags line up their values at this column, as COD files do.
const TAG_WIDTH: usize = 34;

impl Cif {
    fn blank(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    fn pair(&mut self, tag: &str, value: Rendered) {
        self.tags.insert(tag.to_ascii_lowercase());
        match value {
            Rendered::Inline(text) => {
                let _ = writeln!(self.out, "{tag:<width$} {text}", width = TAG_WIDTH - 1);
            }
            Rendered::TextField(text) => {
                let _ = writeln!(self.out, "{tag}\n;{text}\n;");
            }
        }
    }

    /// A loop. Columns line up unless a value needs a text field.
    fn table(&mut self, tags: &[impl AsRef<str>], rows: Vec<Vec<Rendered>>) {
        self.out.push_str("loop_\n");
        for tag in tags {
            self.tags.insert(tag.as_ref().to_ascii_lowercase());
            let _ = writeln!(self.out, "{}", tag.as_ref());
        }
        let mut widths = vec![0; tags.len()];
        for row in &rows {
            for (width, value) in widths.iter_mut().zip(row) {
                if let Rendered::Inline(text) = value {
                    *width = (*width).max(text.chars().count());
                }
            }
        }
        for row in rows {
            let mut line = String::new();
            for (i, value) in row.into_iter().enumerate() {
                match value {
                    Rendered::Inline(text) => {
                        if !line.is_empty() {
                            line.push(' ');
                        }
                        if i + 1 < widths.len() {
                            let _ = write!(line, "{text:<width$}", width = widths[i]);
                        } else {
                            line.push_str(&text);
                        }
                    }
                    Rendered::TextField(text) => {
                        // A text field starts and ends a line.
                        if !line.trim_end().is_empty() {
                            self.out.push_str(line.trim_end());
                            self.out.push('\n');
                        }
                        line.clear();
                        let _ = writeln!(self.out, ";{text}\n;");
                    }
                }
            }
            if !line.trim_end().is_empty() {
                self.out.push_str(line.trim_end());
                self.out.push('\n');
            }
        }
    }
}

/// A value ready to write: inline, bare or quoted, or a text field, which
/// takes whole lines.
#[derive(Clone, Debug, PartialEq)]
enum Rendered {
    Inline(String),
    TextField(String),
}

/// CIF 1.1 lines are at most 2048 characters; longer values go into text
/// fields.
const MAX_INLINE: usize = 2000;

/// A string value, quoted only when it has to be.
fn text(value: &str) -> Rendered {
    if value.contains(['\n', '\r']) || value.chars().count() > MAX_INLINE {
        return Rendered::TextField(value.to_string());
    }
    if is_bare(value) {
        return Rendered::Inline(value.to_string());
    }
    // A quote ends the string only where whitespace follows it.
    for quote in ['\'', '"'] {
        let closes_early = value
            .char_indices()
            .any(|(i, c)| c == quote && value[i + 1..].starts_with([' ', '\t']));
        if !closes_early && !value.ends_with(quote) {
            return Rendered::Inline(format!("{quote}{value}{quote}"));
        }
    }
    Rendered::TextField(value.to_string())
}

/// Whether a value can be written without quotes and still read as the
/// same text: no whitespace, no leading character with a meaning, not a
/// reserved word or a special value.
pub(crate) fn is_bare(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    !value.is_empty()
        && !value.contains(char::is_whitespace)
        && !value.starts_with(['_', '#', '$', '\'', '"', '[', ']', ';'])
        && value != "?"
        && value != "."
        && !["data_", "save_"].iter().any(|p| lower.starts_with(p))
        && !["loop_", "global_", "stop_"].contains(&lower.as_str())
}

/// A kept value as the file wrote it: unquoted values were valid bare.
fn kept_value(value: &Text) -> Rendered {
    if value.quoted {
        text(&value.text)
    } else {
        Rendered::Inline(value.text.clone())
    }
}

/// The block name with every character CIF 1.1 does not allow replaced.
fn block_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_graphic() {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_');
    if out.is_empty() {
        "structure".into()
    } else {
        out.into()
    }
}

/// The decimals a number was written with, if it reads like a decimal
/// with at most [`MAX_DECIMALS`] of them.
fn decimals(values: &[f64]) -> Option<i32> {
    (0..=MAX_DECIMALS).find(|&d| {
        let scale = 10f64.powi(d);
        values.iter().all(|v| is_whole(v * scale))
    })
}

/// A number as it was read, e.g. `4.594`; one that never was a short
/// decimal, such as 1/3 or a U converted from B, with ten decimals.
fn plain(value: f64) -> String {
    let decimals = decimals(&[value]).unwrap_or(MAX_DECIMALS);
    rounded(value, decimals as usize)
}

/// `value` rounded to `decimals`, without trailing zeros.
fn rounded(value: f64, decimals: usize) -> String {
    let text = format!("{:.decimals$}", value + 0.0);
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        &text
    };
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// Whether `x` is a whole number, up to the rounding of reading a decimal.
fn is_whole(x: f64) -> bool {
    (x - x.round()).abs() <= 1e-12 * x.abs().max(1.0)
}

/// Decimals beyond which values read from a file never go.
const MAX_DECIMALS: i32 = 10;

/// A number with its standard uncertainty in parentheses, e.g. `3.905(2)`.
///
/// A value and uncertainty read from a file are written with the decimals
/// they had, so that they read back the same. Others, such as a U
/// converted from B, follow the usual rule: the uncertainty gets two
/// digits up to 19, otherwise one, and the value is rounded to match.
fn number(value: Measured) -> String {
    let x = value.value();
    match value.su() {
        None => plain(x),
        // `90.0(0)`: an uncertainty of zero is still stated.
        Some(0.0) => format!("{}(0)", plain(x)),
        Some(su) => with_su(x, su, written_decimals(value)),
    }
}

/// `x` with `decimals` decimals and its s.u. in the last of them, e.g.
/// `0.69376(50)` for 0.0005 at five decimals.
fn with_su(x: f64, su: f64, decimals: i32) -> String {
    let digits = (su * 10f64.powi(decimals)).round().max(1.0);
    let text = format!("{:.decimals$}", x + 0.0, decimals = decimals as usize);
    // A value that rounds to zero is written without a sign: `-0.000(6)`
    // reads back as zero, which would be written `0.000(6)` the next time.
    let text = match text.strip_prefix('-') {
        Some(rest) if rest.chars().all(|c| c == '0' || c == '.') => rest.to_string(),
        _ => text,
    };
    format!("{text}({digits})")
}

/// The value [`number`] writes, rounded as written.
fn as_written(value: Measured) -> Measured {
    let scale = 10f64.powi(written_decimals(value));
    let x = (value.value() * scale).round() / scale;
    match value.su() {
        Some(su) => Measured::with_su(x, su),
        None => Measured::exact(x),
    }
}

/// The decimals [`number`] writes a value with.
fn written_decimals(value: Measured) -> i32 {
    let x = value.value();
    let Some(su) = value.su().filter(|su| *su > 0.0) else {
        return decimals(&[x]).unwrap_or(MAX_DECIMALS);
    };
    decimals(&[x, su]).unwrap_or_else(|| {
        let magnitude = su.log10().floor() as i32;
        let leading = (su / 10f64.powi(magnitude - 1)).round();
        // Two digits from 10 to 19, one above; `(10)` is `(1)` one decimal
        // up, which is how it reads back.
        let decimals = if leading <= 19.0 && leading != 10.0 {
            1 - magnitude
        } else {
            // One digit, unless it rounds up to 10, as 0.000975 does at four
            // decimals: that too is `(1)` one decimal up, or the next file
            // written from the reread value would differ.
            let one = -magnitude;
            if (su * 10f64.powi(one)).round() >= 10.0 {
                one - 1
            } else {
                one
            }
        };
        decimals.max(0)
    })
}

fn write_cell(cif: &mut Cif, structure: &Structure, orbits: &[mw_crystal::Orbit]) {
    let params = structure.lattice.params();
    for (tag, value) in [
        ("_cell_length_a", params.a),
        ("_cell_length_b", params.b),
        ("_cell_length_c", params.c),
        ("_cell_angle_alpha", params.alpha),
        ("_cell_angle_beta", params.beta),
        ("_cell_angle_gamma", params.gamma),
    ] {
        cif.pair(tag, Rendered::Inline(number(value)));
    }
    cif.pair(
        "_cell_volume",
        Rendered::Inline(rounded(params.volume(), 3)),
    );
    if let Some((formula, z)) = formula(structure, orbits) {
        cif.pair("_cell_formula_units_Z", Rendered::Inline(z.to_string()));
        cif.pair("_chemical_formula_sum", text(&formula));
    }
}

/// The formula in Hill order and the formula units per cell. Z is the
/// largest common divisor of the atom counts when they are whole, else of
/// the orbit sizes, so that partially occupied sites keep their
/// fractions: `Ba0.5 O3 Sr0.5 Ti`, Z = 1.
fn formula(structure: &Structure, orbits: &[mw_crystal::Orbit]) -> Option<(String, u64)> {
    let mut counts: BTreeMap<&str, f64> = BTreeMap::new();
    for orbit in orbits {
        let site = structure.site(orbit.site)?;
        for occupant in &site.occupants {
            *counts.entry(occupant.species.element.as_str()).or_default() +=
                orbit.positions.len() as f64 * occupant.occupancy.value();
        }
    }
    counts.retain(|_, count| *count > 0.0);
    if counts.is_empty() {
        return None;
    }
    let whole = counts.values().all(|c| (c - c.round()).abs() < 1e-6);
    let divisors: Vec<u64> = if whole {
        counts.values().map(|c| c.round() as u64).collect()
    } else {
        orbits.iter().map(|o| o.positions.len() as u64).collect()
    };
    let z = divisors.into_iter().fold(0, gcd).max(1);
    // Hill order: C, then H, then the rest alphabetically; without carbon,
    // everything alphabetically.
    let mut elements: Vec<&str> = counts.keys().copied().collect();
    if counts.contains_key("C") {
        elements.sort_by_key(|e| match *e {
            "C" => (0, *e),
            "H" => (1, *e),
            _ => (2, *e),
        });
    }
    let parts: Vec<String> = elements
        .iter()
        .map(|e| {
            let count = counts[e] / z as f64;
            if (count - 1.0).abs() < 1e-6 {
                e.to_string()
            } else {
                format!("{e}{}", rounded(count, 4))
            }
        })
        .collect();
    Some((parts.join(" "), z))
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The Hermann–Mauguin symbol that names the operations' setting, as CIF
/// writes it (`P 42/m n m`): the full symbol for a monoclinic setting
/// other than the standard one, the short symbol otherwise, with the
/// origin choice or axes when the setting has a choice and is not the
/// standard one.
fn hm_symbol(space_group: &SpaceGroupType) -> Option<String> {
    let symbol = match &space_group.setting {
        Setting::Standard => space_group.hm_symbol.to_string(),
        Setting::Tabulated { code, .. } if code.starts_with(['1', '2', 'H', 'R']) => {
            format!("{} :{}", space_group.hm_symbol, &code[..1])
        }
        Setting::Tabulated { symbol, .. }
            if space_group.crystal_system() == CrystalSystem::Monoclinic =>
        {
            symbol.to_string()
        }
        Setting::Tabulated { .. } => space_group.hm_symbol.to_string(),
        // Its symbol names another setting than the operations'.
        Setting::Transformed => return None,
    };
    Some(symbol.replace('_', ""))
}

fn crystal_system(system: CrystalSystem) -> &'static str {
    match system {
        CrystalSystem::Triclinic => "triclinic",
        CrystalSystem::Monoclinic => "monoclinic",
        CrystalSystem::Orthorhombic => "orthorhombic",
        CrystalSystem::Tetragonal => "tetragonal",
        CrystalSystem::Trigonal => "trigonal",
        CrystalSystem::Hexagonal => "hexagonal",
        CrystalSystem::Cubic => "cubic",
    }
}

fn write_space_group(cif: &mut Cif, structure: &Structure, space_group: Option<&SpaceGroupType>) {
    if let Some(space_group) = space_group {
        cif.pair(
            "_space_group_crystal_system",
            Rendered::Inline(crystal_system(space_group.crystal_system()).into()),
        );
        cif.pair(
            "_space_group_IT_number",
            Rendered::Inline(space_group.number.to_string()),
        );
        if let Some(symbol) = hm_symbol(space_group) {
            cif.pair("_space_group_name_H-M_alt", quoted(&symbol));
        }
        if let Some(hall) = space_group.hall_symbol() {
            cif.pair("_space_group_name_Hall", quoted(hall));
        }
    }
    cif.blank();
    let rows = structure
        .symmetry
        .operations()
        .iter()
        .zip(1..)
        .map(|(op, id)| vec![Rendered::Inline(id.to_string()), quoted(&op.to_string())])
        .collect();
    cif.table(
        &["_space_group_symop_id", "_space_group_symop_operation_xyz"],
        rows,
    );
}

/// Quoted even when it need not be, for symbols with spaces in some
/// settings but not others.
fn quoted(value: &str) -> Rendered {
    match text(value) {
        Rendered::Inline(text) if !text.starts_with(['\'', '"']) => {
            Rendered::Inline(format!("'{text}'"))
        }
        other => other,
    }
}

/// Ueq is derived; more decimals would only show rounding.
const UEQ_DECIMALS: i32 = 6;

/// Ueq: a third of the trace of U in Cartesian axes,
/// `⅓ Σᵢⱼ Uⁱʲ aᵢ* aⱼ* (aᵢ·aⱼ)`.
fn u_equivalent(structure: &Structure, u: &[Measured; 6]) -> f64 {
    let params = structure.lattice.params();
    let lengths = [params.a, params.b, params.c].map(|l| l.value());
    let cos = [params.alpha, params.beta, params.gamma].map(|a| a.value().to_radians().cos());
    // G_ij = a_i·a_j; the angle between a_i and a_j is opposite the third axis.
    let angle = |i: usize, j: usize| if i == j { 1.0 } else { cos[3 - i - j] };
    let g = [0, 1, 2].map(|i| [0, 1, 2].map(|j| lengths[i] * lengths[j] * angle(i, j)));
    let det = g[0][0] * (g[1][1] * g[2][2] - g[1][2] * g[2][1])
        - g[0][1] * (g[1][0] * g[2][2] - g[1][2] * g[2][0])
        + g[0][2] * (g[1][0] * g[2][1] - g[1][1] * g[2][0]);
    // Diagonal of G⁻¹: the squared reciprocal lengths.
    let reciprocal = [
        ((g[1][1] * g[2][2] - g[1][2] * g[2][1]) / det).sqrt(),
        ((g[0][0] * g[2][2] - g[0][2] * g[2][0]) / det).sqrt(),
        ((g[0][0] * g[1][1] - g[0][1] * g[1][0]) / det).sqrt(),
    ];
    let [u11, u22, u33, u12, u13, u23] = u.map(|m| m.value());
    let tensor = [[u11, u12, u13], [u12, u22, u23], [u13, u23, u33]];
    let mut sum = 0.0;
    for i in 0..3 {
        for j in 0..3 {
            sum += tensor[i][j] * reciprocal[i] * reciprocal[j] * g[i][j];
        }
    }
    sum / 3.0
}

fn write_sites(
    cif: &mut Cif,
    structure: &Structure,
    analysis: &SymmetryAnalysis,
    orbits: &[mw_crystal::Orbit],
    kept: Option<&CifFields>,
    changed: bool,
) {
    let tabulated = analysis
        .space_group
        .as_ref()
        .is_some_and(|sg| sg.setting != Setting::Transformed);
    let symmetry = |site| {
        analysis
            .sites
            .iter()
            .find_map(|(id, symmetry)| (*id == site).then_some(symmetry))
    };
    let occupants = || {
        structure
            .sites
            .iter()
            .flat_map(|site| site.occupants.iter().map(move |o| (site, o)))
    };
    let wyckoff = tabulated
        && structure
            .sites
            .iter()
            .any(|site| symmetry(site.id).is_some_and(|s| s.wyckoff.is_some()));
    let adp = occupants().any(|(_, o)| o.adp.is_some());
    let disorder = structure.sites.iter().any(|site| site.disorder.is_some());
    let extra: &[Column] = match kept {
        Some(kept) if !changed => &kept.site_columns,
        _ => &[],
    };

    let mut tags = vec![
        "_atom_site_label",
        "_atom_site_type_symbol",
        "_atom_site_symmetry_multiplicity",
    ];
    if wyckoff {
        tags.push("_atom_site_Wyckoff_symbol");
    }
    tags.extend([
        "_atom_site_fract_x",
        "_atom_site_fract_y",
        "_atom_site_fract_z",
        "_atom_site_occupancy",
    ]);
    if adp {
        tags.extend(["_atom_site_U_iso_or_equiv", "_atom_site_adp_type"]);
    }
    if disorder {
        tags.extend(["_atom_site_disorder_assembly", "_atom_site_disorder_group"]);
    }
    tags.extend(extra.iter().map(|c| c.tag.as_str()));

    // Per axis, the decimals that keep rounding below a quarter of the
    // position tolerance: 0.5 × 10⁻ᵈ × length ≤ tolerance / 4.
    let params = structure.lattice.params();
    let tolerance = structure.tolerances.position;
    let tie_decimals = [params.a, params.b, params.c].map(|length| {
        ((2.0 * length.value() / tolerance).log10().ceil() as i32).clamp(0, MAX_DECIMALS)
    });

    let unknown = || Rendered::Inline("?".into());
    let inapplicable = || Rendered::Inline(".".into());
    let mut rows = Vec::new();
    for (site, occupant) in occupants() {
        let site_symmetry = symmetry(site.id);
        let multiplicity = orbits
            .iter()
            .find(|o| o.site == site.id)
            .map_or(0, |o| o.positions.len());
        let mut row = vec![
            text(&occupant.label),
            text(&occupant.species.symbol),
            Rendered::Inline(multiplicity.to_string()),
        ];
        if wyckoff {
            row.push(
                site_symmetry
                    .and_then(|s| s.wyckoff.as_ref())
                    .map_or_else(unknown, |w| Rendered::Inline(w.letter.to_string())),
            );
        }
        let constraints = site_symmetry.map(|s| s.coordinates);
        let coordinates = [0, 1, 2].map(|axis| {
            // A coordinate fixed by symmetry is exact.
            let fixed = constraints.is_some_and(|c| c[axis] == CoordinateConstraint::Fixed);
            match site.position_su[axis].filter(|_| !fixed) {
                Some(su) => Measured::with_su(site.position[axis], su),
                None => Measured::exact(site.position[axis]),
            }
        });
        // Coordinates that follow one another put the site on a special
        // position; reading the file back snaps it there again from their
        // average. Each is written precisely enough that rounding moves the
        // atom by at most a quarter of the position tolerance, and at least
        // as precisely as the coordinate it follows, so that the average,
        // and the next file written from it, stay the same.
        let pivot_of = |axis: usize| match constraints.map(|c| c[axis]) {
            Some(CoordinateConstraint::Follows(pivot)) => Some(pivot as usize),
            _ => None,
        };
        let tied = [0, 1, 2].map(|axis| {
            pivot_of(axis).is_some() || (0..3).any(|other| pivot_of(other) == Some(axis))
        });
        let decimals = |axis: usize| {
            let written = written_decimals(coordinates[axis]);
            if tied[axis] {
                written.max(tie_decimals[axis])
            } else {
                written
            }
        };
        for (axis, coordinate) in coordinates.iter().enumerate() {
            let text = match (coordinate.su(), pivot_of(axis)) {
                // Usually given without an s.u.: rounded like the one it
                // follows, since its own value holds float noise from the
                // average.
                (None, Some(pivot)) if coordinates[pivot].su().is_some() => {
                    rounded(coordinate.value(), decimals(pivot) as usize)
                }
                // With an s.u., its digits scaled to the decimals, e.g.
                // `0.2312(100)` for 0.010 at four decimals.
                (Some(su), pivot) if su > 0.0 && tied[axis] => {
                    let at_least = pivot.map_or(0, decimals);
                    with_su(coordinate.value(), su, decimals(axis).max(at_least))
                }
                _ => number(*coordinate),
            };
            row.push(Rendered::Inline(text));
        }
        row.push(Rendered::Inline(number(occupant.occupancy)));
        if adp {
            match &occupant.adp {
                Some(Adp::Isotropic(u)) => {
                    row.push(Rendered::Inline(number(*u)));
                    row.push(Rendered::Inline("Uiso".into()));
                }
                Some(Adp::Anisotropic(u)) => {
                    // From the components as written, and as precise as
                    // they are up to 10⁻⁶ Å², so that reading the file
                    // back gives the same Ueq even where writing rounded
                    // them (a U converted from B).
                    let decimals = u.iter().map(|m| written_decimals(*m)).max();
                    let decimals = decimals.unwrap_or(UEQ_DECIMALS).min(UEQ_DECIMALS);
                    let ueq = u_equivalent(structure, &u.map(as_written));
                    row.push(Rendered::Inline(rounded(ueq, decimals as usize)));
                    row.push(Rendered::Inline("Uani".into()));
                }
                None => row.extend([unknown(), unknown()]),
            }
        }
        if disorder {
            match &site.disorder {
                Some(d) => {
                    row.push(d.assembly.as_deref().map_or_else(inapplicable, text));
                    row.push(text(&d.group));
                }
                None => row.extend([inapplicable(), inapplicable()]),
            }
        }
        for column in extra {
            row.push(
                column
                    .values
                    .get(&occupant.label)
                    .map_or_else(unknown, kept_value),
            );
        }
        rows.push(row);
    }
    cif.blank();
    cif.table(&tags, rows);

    let anisotropic: Vec<_> = occupants()
        .filter_map(|(_, o)| match &o.adp {
            Some(Adp::Anisotropic(u)) => Some((o, u)),
            _ => None,
        })
        .collect();
    if anisotropic.is_empty() {
        return;
    }
    let extra: &[Column] = match kept {
        Some(kept) if !changed => &kept.aniso_columns,
        _ => &[],
    };
    let mut tags = vec![
        "_atom_site_aniso_label",
        "_atom_site_aniso_U_11",
        "_atom_site_aniso_U_22",
        "_atom_site_aniso_U_33",
        "_atom_site_aniso_U_12",
        "_atom_site_aniso_U_13",
        "_atom_site_aniso_U_23",
    ];
    tags.extend(extra.iter().map(|c| c.tag.as_str()));
    let rows = anisotropic
        .into_iter()
        .map(|(occupant, u)| {
            let mut row = vec![text(&occupant.label)];
            row.extend(u.iter().map(|m| Rendered::Inline(number(*m))));
            for column in extra {
                row.push(
                    column
                        .values
                        .get(&occupant.label)
                        .map_or_else(unknown, kept_value),
                );
            }
            row
        })
        .collect();
    cif.blank();
    cif.table(&tags, rows);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_keep_their_decimals() {
        let n = |value, su| number(Measured::with_su(value, su));
        assert_eq!(n(3.905, 0.002), "3.905(2)");
        assert_eq!(n(0.30478, 0.00012), "0.30478(12)");
        assert_eq!(n(3.9050, 0.0040), "3.905(4)");
        assert_eq!(n(3.9052, 0.0040), "3.9052(40)");
        assert_eq!(n(120.0, 30.0), "120(30)");
        assert_eq!(n(1.23e-4, 5e-6), "0.000123(5)");
        assert_eq!(number(Measured::exact(90.0)), "90");
        assert_eq!(number(Measured::exact(-0.0)), "0");
        assert_eq!(n(90.0, 0.0), "90(0)");
        assert_eq!(number(Measured::exact(1.0 / 3.0)), "0.3333333333");
        assert_eq!(
            number(Measured::exact(0.3049 / 2.0 + 0.3047 / 2.0)),
            "0.3048"
        );
    }

    #[test]
    fn computed_uncertainties_follow_the_rule_of_19() {
        let n = |value, su| number(Measured::with_su(value, su));
        // U from B = 0.79(8): 0.010005… ± 0.001013…
        let b_to_u = 8.0 * std::f64::consts::PI.powi(2);
        assert_eq!(n(0.79 / b_to_u, 0.08 / b_to_u), "0.010(1)");
        assert_eq!(n(0.79 / b_to_u, 0.09 / b_to_u), "0.0100(11)");
        assert_eq!(n(10.0 / 81.0, 2f64.sqrt() / 563.0), "0.123(3)");
        assert_eq!(n(1000.0 / 81.0, 1.52 / 81.0), "12.346(19)");
    }

    #[test]
    fn values_are_quoted_only_when_needed() {
        let inline = |value: &str| match text(value) {
            Rendered::Inline(text) => text,
            Rendered::TextField(text) => panic!("text field for {text:?}"),
        };
        assert_eq!(inline("Rutile"), "Rutile");
        assert_eq!(inline("P 42/m n m"), "'P 42/m n m'");
        assert_eq!(inline("it's"), "it's");
        assert_eq!(inline("it' s"), "\"it' s\"");
        assert_eq!(inline("?"), "'?'");
        assert_eq!(inline("_x"), "'_x'");
        assert_eq!(inline("data_x"), "'data_x'");
        assert_eq!(inline(""), "''");
        assert_eq!(
            text("a\nb"),
            Rendered::TextField("a\nb".into()),
            "multi-line text"
        );
        assert_eq!(
            text("'a' \"b\" c'"),
            Rendered::TextField("'a' \"b\" c'".into())
        );
    }

    #[test]
    fn block_names() {
        assert_eq!(block_name("rutile"), "rutile");
        assert_eq!(block_name("TiO2 rutile"), "TiO2_rutile");
        assert_eq!(block_name("金红石"), "structure");
        assert_eq!(block_name(""), "structure");
    }
}
