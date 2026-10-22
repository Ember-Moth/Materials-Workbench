//! The atoms of a structure one by one, for the formats that list atoms
//! instead of sites and operations (POSCAR, extended XYZ): the structure
//! expanded to P1, with one element per position.

use std::fmt;

use mw_crystal::{Structure, UNKNOWN_ELEMENT};

use crate::normalize::OCCUPANCY_TOLERANCE;

/// Why a structure cannot be written as a list of atoms.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExportError {
    /// Sites that are partially occupied or shared by several species,
    /// by label. A list of atoms has one element on each position; which
    /// atoms to keep is an ordering only the user can choose (design §9.3).
    PartialOccupancy { labels: Vec<String> },
    /// Sites whose element is unknown ([`UNKNOWN_ELEMENT`]), by label.
    UnknownElement { labels: Vec<String> },
    /// The structure has no sites.
    NoAtoms,
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PartialOccupancy { labels } => write!(
                f,
                "partially occupied or mixed sites need ordering first: {}",
                labels.join(", ")
            ),
            Self::UnknownElement { labels } => {
                write!(f, "sites with an unknown element: {}", labels.join(", "))
            }
            Self::NoAtoms => write!(f, "the structure has no atoms"),
        }
    }
}

impl std::error::Error for ExportError {}

/// Whether every site holds one fully occupied, known element.
pub(crate) fn check(structure: &Structure) -> Result<(), ExportError> {
    if structure.sites.is_empty() {
        return Err(ExportError::NoAtoms);
    }
    let partial: Vec<String> = structure
        .sites
        .iter()
        .filter(|site| {
            let occupancy: f64 = site.occupants.iter().map(|o| o.occupancy.value()).sum();
            site.occupants.len() != 1 || occupancy < 1.0 - OCCUPANCY_TOLERANCE
        })
        .map(|site| site.label().to_string())
        .collect();
    if !partial.is_empty() {
        return Err(ExportError::PartialOccupancy { labels: partial });
    }
    let unknown: Vec<String> = structure
        .sites
        .iter()
        .filter(|site| site.element() == UNKNOWN_ELEMENT)
        .map(|site| site.label().to_string())
        .collect();
    if !unknown.is_empty() {
        return Err(ExportError::UnknownElement { labels: unknown });
    }
    Ok(())
}

/// Atoms by element: the element and the atoms' fractional coordinates.
pub(crate) type ElementGroups<'a> = Vec<(&'a str, Vec<[f64; 3]>)>;

/// The atoms of the unit cell by element, the elements in the order they
/// first occur among the sites, each element's atoms in site order.
/// Fractional coordinates in [0, 1).
pub(crate) fn by_element(structure: &Structure) -> Result<ElementGroups<'_>, ExportError> {
    check(structure)?;
    let mut groups: ElementGroups = Vec::new();
    for (site, orbit) in structure.sites.iter().zip(mw_crystal::orbits(structure)) {
        let element = site.element();
        match groups.iter_mut().find(|(e, _)| *e == element) {
            Some((_, atoms)) => atoms.extend(orbit.positions),
            None => groups.push((element, orbit.positions)),
        }
    }
    Ok(groups)
}

/// A number with ten decimals, without a sign on zero.
pub(crate) fn fixed(value: f64, width: usize) -> String {
    let value = if value.abs() < 5e-11 { 0.0 } else { value };
    format!("{value:>width$.10}")
}
