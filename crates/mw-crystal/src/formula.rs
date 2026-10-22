//! The chemical formula (design §5.3): the cell contents, Σ orbit size ×
//! occupancy per element, and the formula units per cell.

use std::collections::BTreeMap;
use std::fmt;

use crate::{Derived, Structure};

/// Amounts within this of a whole number count as whole.
const WHOLE: f64 = 1e-6;

#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Formula {
    /// Element symbols and their amounts per unit cell, in Hill order:
    /// carbon, hydrogen, then the rest alphabetically; alphabetically when
    /// there is no carbon.
    pub cell: Vec<(String, f64)>,
    /// Formula units per cell, Z: the greatest common divisor of the
    /// amounts when all of them are whole numbers, else 1.
    pub z: u32,
}

impl Formula {
    pub fn of(structure: &Structure, derived: &Derived) -> Self {
        let mut amounts: BTreeMap<&str, f64> = BTreeMap::new();
        for orbit in &derived.orbits {
            let Some(site) = structure.site(orbit.site) else {
                continue;
            };
            for occupant in &site.occupants {
                *amounts.entry(&occupant.species.element).or_default() +=
                    orbit.positions.len() as f64 * occupant.occupancy.value();
            }
        }
        let carbon = amounts.contains_key("C");
        let mut cell: Vec<(String, f64)> = amounts
            .into_iter()
            .map(|(element, amount)| (element.to_string(), amount))
            .collect();
        cell.sort_by_key(|(element, _)| match element.as_str() {
            "C" if carbon => (0, String::new()),
            "H" if carbon => (1, String::new()),
            _ => (2, element.clone()),
        });
        let whole: Option<Vec<u64>> = cell
            .iter()
            .map(|(_, amount)| {
                let rounded = amount.round();
                ((amount - rounded).abs() < WHOLE && rounded >= 1.0).then_some(rounded as u64)
            })
            .collect();
        let z = whole
            .and_then(|amounts| amounts.into_iter().reduce(gcd))
            .map_or(1, |z| z as u32);
        Self { cell, z }
    }

    /// Amounts per formula unit, in Hill order.
    pub fn units(&self) -> impl Iterator<Item = (&str, f64)> {
        let z = f64::from(self.z);
        self.cell
            .iter()
            .map(move |(element, amount)| (element.as_str(), amount / z))
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Up to four decimals, without trailing zeros.
fn amount(value: f64) -> String {
    let text = format!("{value:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl fmt::Display for Formula {
    /// Per formula unit, e.g. `TiO2` or `Fe0.9Ni0.1O`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (element, count) in self.units() {
            f.write_str(element)?;
            if (count - 1.0).abs() >= WHOLE {
                f.write_str(&amount(count))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{self, SampleKey};
    use crate::{Measured, Occupant, Species};

    fn formula(structure: &Structure) -> Formula {
        Formula::of(structure, &Derived::compute(structure))
    }

    #[test]
    fn samples() {
        let rutile = sample::load(SampleKey::Rutile).structure;
        let f = formula(&rutile);
        assert_eq!((f.to_string(), f.z), ("O2Ti".to_string(), 2));
        assert_eq!(f.cell, [("O".into(), 4.0), ("Ti".into(), 2.0)]);
        let supercell = sample::load(SampleKey::RutileSupercell).structure;
        assert_eq!(formula(&supercell).z, 16);
        let srtio3 = sample::load(SampleKey::StrontiumTitanate).structure;
        assert_eq!(formula(&srtio3).to_string(), "O3SrTi");
    }

    #[test]
    fn partial_occupancy_and_hill_order() {
        let mut rutile = sample::load(SampleKey::Rutile).structure;
        rutile.sites[0].occupants[0].occupancy = Measured::exact(0.9);
        rutile.sites[0].occupants.push(Occupant::new(
            "C1",
            Species::element("C"),
            Measured::exact(0.1),
        ));
        let f = formula(&rutile);
        assert_eq!(f.z, 1);
        assert_eq!(f.to_string(), "C0.2O4Ti1.8");
        assert!(
            formula(&crate::Structure::new(
                rutile.lattice,
                rutile.symmetry.clone(),
                Vec::new()
            ))
            .cell
            .is_empty()
        );
    }
}
