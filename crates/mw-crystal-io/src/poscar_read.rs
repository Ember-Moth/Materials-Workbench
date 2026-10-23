//! VASP POSCAR and CONTCAR import (design §9.3): a comment line, the
//! scaling factor, three lattice vectors, the element line (VASP 5), the
//! atom counts, an optional selective dynamics line, Direct or Cartesian,
//! then one line of coordinates per atom. The result is a P1 structure
//! whose lattice keeps the vectors' orientation (design §4.2).
//!
//! Lines are read by position, as VASP reads them; numbers after the ones
//! a line needs (selective dynamics flags, comments) are ignored.

use std::borrow::Cow;

use mw_crystal::{
    Lattice, Measured, Occupant, Site, SiteId, Species, Structure, Symmetry, UNKNOWN_ELEMENT,
    element,
};

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::{CifFields, ImportReport, ImportedBlock};

/// Reads a POSCAR or CONTCAR file's bytes. Invalid UTF-8 is replaced and
/// reported. A VASP 4 file, which names no elements, is an error that says
/// how many to give; [`read_with_elements`] reads it with them.
pub fn read(bytes: &[u8]) -> ImportReport {
    read_with_elements(bytes, &[])
}

/// [`read`] with the elements of a VASP 4 file, one per atom count, in
/// order. They are ignored when the file has an element line.
pub fn read_with_elements(bytes: &[u8], elements: &[&str]) -> ImportReport {
    let (text, valid) = match std::str::from_utf8(bytes) {
        Ok(text) => (Cow::Borrowed(text), true),
        Err(_) => (String::from_utf8_lossy(bytes), false),
    };
    let mut report = read_str_with_elements(&text, elements);
    if !valid {
        report
            .diagnostics
            .insert(0, Diagnostic::new(DiagnosticKind::NotUtf8, None));
    }
    report
}

/// Reads POSCAR text.
pub fn read_str(text: &str) -> ImportReport {
    read_str_with_elements(text, &[])
}

fn read_str_with_elements(text: &str, elements: &[&str]) -> ImportReport {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut diagnostics = Vec::new();
    let structure = parse(&lines, elements, &mut diagnostics);
    // Stable: diagnostics on one line keep the order they were found in.
    diagnostics.sort_by_key(|d| d.line().unwrap_or(u32::MAX));
    ImportReport {
        blocks: vec![ImportedBlock {
            // The comment line names the structure, as VASP's own tools use it.
            name: lines.first().map_or("", |l| l.trim()).to_string(),
            line: 1,
            structure,
            diagnostics,
            fields: CifFields::default(),
        }],
        diagnostics: Vec::new(),
    }
}

/// The lines of the file with their 1-based numbers, read in order.
struct Lines<'a> {
    lines: &'a [&'a str],
    next: usize,
}

impl<'a> Lines<'a> {
    fn next(&mut self) -> Result<(u32, &'a str), Diagnostic> {
        let line = self
            .lines
            .get(self.next)
            .ok_or_else(|| Diagnostic::new(DiagnosticKind::PoscarTruncated, None))?;
        self.next += 1;
        Ok((self.next as u32, line))
    }
}

/// A real number as Fortran writes it, `1.5D-3` included; never infinite
/// or NaN.
fn real(token: &str) -> Option<f64> {
    let value: f64 = token.replace(['D', 'd'], "e").parse().ok()?;
    value.is_finite().then_some(value)
}

/// The first `n` tokens of a line as real numbers, if they all are.
fn reals<const N: usize>(line: &str) -> Option<[f64; N]> {
    let mut tokens = line.split_whitespace();
    let mut values = [0.0; N];
    for value in &mut values {
        *value = real(tokens.next()?)?;
    }
    Some(values)
}

/// The leading tokens of a line that are integers.
fn counts(line: &str) -> Vec<usize> {
    line.split_whitespace()
        .map_while(|token| token.parse::<usize>().ok())
        .collect()
}

/// How the second line scales lengths.
#[derive(Clone, Copy)]
enum Scale {
    Factor(f64),
    /// A negative factor: the cell volume in Å³.
    Volume(f64),
}

fn scale(line: &str, number: u32) -> Result<Scale, Diagnostic> {
    let values: Vec<f64> = line.split_whitespace().take(3).map_while(real).collect();
    let invalid = || {
        Diagnostic::at(
            DiagnosticKind::InvalidScale {
                text: line.trim().to_string(),
            },
            number,
        )
    };
    match values[..] {
        [s] if s > 0.0 => Ok(Scale::Factor(s)),
        [s] if s < 0.0 => Ok(Scale::Volume(-s)),
        // Three factors scale the Cartesian axes; equal ones are one factor.
        [a, b, c] if a > 0.0 && b > 0.0 && c > 0.0 => {
            if a == b && b == c {
                Ok(Scale::Factor(a))
            } else {
                Err(Diagnostic::at(
                    DiagnosticKind::UnequalScaleFactors { factors: [a, b, c] },
                    number,
                ))
            }
        }
        _ => Err(invalid()),
    }
}

fn determinant(m: &[[f64; 3]; 3]) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// What an element line or the user names: the element, or the unknown
/// element with the name kept as its symbol. POTCAR suffixes such as
/// `Fe_pv` or the `/hash` VASP 6 appends are not part of the element.
fn species_named(name: &str) -> (Species, bool) {
    let letters: String = name.chars().take_while(char::is_ascii_alphabetic).collect();
    let mut chars = letters.chars();
    let symbol: String = chars
        .next()
        .map(|first| first.to_ascii_uppercase())
        .into_iter()
        .chain(chars.map(|c| c.to_ascii_lowercase()))
        .collect();
    if element::is_element(&symbol) {
        (Species::element(&symbol), true)
    } else {
        (Species::new(UNKNOWN_ELEMENT, None, name), false)
    }
}

fn parse(lines: &[&str], given: &[&str], diagnostics: &mut Vec<Diagnostic>) -> Option<Structure> {
    match parse_or_fail(lines, given, diagnostics) {
        Ok(structure) => structure,
        Err(error) => {
            diagnostics.push(error);
            None
        }
    }
}

/// Reads the file; `Err` for an error that ends reading, `Ok(None)` when
/// errors were reported but reading went on (bad coordinate lines).
fn parse_or_fail(
    lines: &[&str],
    given: &[&str],
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Option<Structure>, Diagnostic> {
    let mut lines = Lines { lines, next: 0 };
    lines.next()?;
    let (number, line) = lines.next()?;
    let scale = scale(line, number)?;

    let mut basis = [[0.0; 3]; 3];
    for vector in &mut basis {
        let (number, line) = lines.next()?;
        *vector = reals::<3>(line).ok_or_else(|| {
            Diagnostic::at(
                DiagnosticKind::InvalidLatticeVector {
                    text: line.trim().to_string(),
                },
                number,
            )
        })?;
    }
    // Lines are read by position: the vectors are on lines 3 to 5.
    let first_vector_line = 3;
    let degenerate = || Diagnostic::at(DiagnosticKind::DegenerateLattice, first_vector_line);
    let factor = match scale {
        Scale::Factor(s) => s,
        Scale::Volume(volume) => {
            let det = determinant(&basis).abs();
            if det == 0.0 {
                return Err(degenerate());
            }
            (volume / det).cbrt()
        }
    };
    let basis = basis.map(|v| v.map(|x| x * factor));
    let lattice = Lattice::from_matrix(basis).ok_or_else(degenerate)?;
    if lattice.is_left_handed() {
        diagnostics.push(Diagnostic::at(
            DiagnosticKind::LeftHandedLattice,
            first_vector_line,
        ));
    }

    // VASP 5 names the elements before the counts; VASP 4 does not.
    let (number, line) = lines.next()?;
    let names: Vec<&str>;
    let names_line;
    let (counts_number, counts_line) = if counts(line).is_empty() {
        names = line.split_whitespace().collect();
        names_line = Some(number);
        lines.next()?
    } else {
        names = given.to_vec();
        names_line = None;
        (number, line)
    };
    let counts = counts(counts_line);
    // Counts too large to add up hold no atoms this file could list.
    let total = counts.iter().try_fold(0usize, |sum, &n| sum.checked_add(n));
    let (false, Some(total)) = (counts.is_empty(), total) else {
        return Err(Diagnostic::at(
            DiagnosticKind::InvalidAtomCounts {
                text: counts_line.trim().to_string(),
            },
            counts_number,
        ));
    };
    if names_line.is_none() && given.is_empty() {
        return Err(Diagnostic::at(
            DiagnosticKind::MissingElements {
                species: counts.len(),
            },
            counts_number,
        ));
    }
    if names.len() != counts.len() {
        return Err(Diagnostic::at(
            DiagnosticKind::SpeciesCountMismatch {
                elements: names.len(),
                counts: counts.len(),
            },
            counts_number,
        ));
    }
    if total == 0 {
        return Err(Diagnostic::at(
            DiagnosticKind::MissingAtomSites,
            counts_number,
        ));
    }

    let (mut number, mut line) = lines.next()?;
    if line.trim_start().starts_with(['S', 's']) {
        diagnostics.push(Diagnostic::at(
            DiagnosticKind::SelectiveDynamicsIgnored,
            number,
        ));
        (number, line) = lines.next()?;
    }
    let mode = line.trim_start();
    if mode.starts_with(|c: char| c.is_ascii_digit() || "+-.".contains(c)) {
        return Err(Diagnostic::at(
            DiagnosticKind::InvalidCoordinateMode {
                text: line.trim().to_string(),
            },
            number,
        ));
    }
    // VASP's rule: C or K is Cartesian, anything else direct.
    let cartesian = mode.starts_with(['C', 'c', 'K', 'k']);

    // Never more atoms than the file has lines, whatever the counts say.
    let capacity = total.min(lines.lines.len());
    let mut sites = Vec::with_capacity(capacity);
    let mut site_lines = Vec::with_capacity(capacity);
    let mut read = 0;
    let mut failed = false;
    // Atoms are labeled by element and counted per element across groups,
    // so that a file listing an element twice still has unique labels.
    let mut counted: Vec<(String, usize)> = Vec::new();
    for (name, &count) in names.iter().zip(&counts) {
        let (species, known) = species_named(name);
        let prefix = if known {
            species.element.clone()
        } else {
            name.to_string()
        };
        let start = match counted.iter_mut().find(|(p, _)| *p == prefix) {
            Some((_, n)) => std::mem::replace(n, *n + count),
            None => {
                counted.push((prefix.clone(), count));
                0
            }
        };
        if !known && count > 0 {
            diagnostics.push(Diagnostic::new(
                DiagnosticKind::UnknownElement {
                    label: format!("{prefix}{}", start + 1),
                    symbol: name.to_string(),
                },
                names_line,
            ));
        }
        for k in 0..count {
            let (number, line) = lines.next().map_err(|_| {
                Diagnostic::new(
                    DiagnosticKind::MissingCoordinates {
                        expected: total,
                        found: read,
                    },
                    u32::try_from(lines.lines.len()).ok(),
                )
            })?;
            read += 1;
            let Some(x) = reals::<3>(line) else {
                diagnostics.push(Diagnostic::at(
                    DiagnosticKind::InvalidCoordinates {
                        text: line.trim().to_string(),
                    },
                    number,
                ));
                failed = true;
                continue;
            };
            let position = if cartesian {
                lattice.to_fractional(x.map(|v| v * factor))
            } else {
                x
            };
            let label = format!("{prefix}{}", start + k + 1);
            let occupant = Occupant::new(&label, species.clone(), Measured::exact(1.0));
            sites.push(Site::new(
                SiteId(sites.len() as u32 + 1),
                position,
                vec![occupant],
            ));
            site_lines.push(number);
        }
    }
    if let Some(number) = (lines.next..lines.lines.len())
        .find(|&i| !lines.lines[i].trim().is_empty())
        .map(|i| i as u32 + 1)
    {
        diagnostics.push(Diagnostic::at(
            DiagnosticKind::TrailingContentIgnored,
            number,
        ));
    }
    if failed {
        return Ok(None);
    }

    let structure = Structure::new(lattice, Symmetry::p1(), sites);
    check_contacts(&structure, &site_lines, diagnostics);
    Ok(Some(structure))
}

/// Atoms closer than the short-contact distance, the same position
/// included: a file lists every atom, so none of them is a repeat to drop.
fn check_contacts(structure: &Structure, lines: &[u32], diagnostics: &mut Vec<Diagnostic>) {
    let orbits = mw_crystal::orbits(structure);
    let short = structure.tolerances.short_contact;
    let index = |id: SiteId| id.0 as usize - 1;
    for contact in mw_crystal::close_contacts(structure, &orbits, short) {
        let (a, b) = (index(contact.a), index(contact.b));
        diagnostics.push(Diagnostic::at(
            DiagnosticKind::ShortContact {
                a: structure.sites[a].label().to_string(),
                b: structure.sites[b].label().to_string(),
                distance: contact.distance,
            },
            lines[b],
        ));
    }
}
