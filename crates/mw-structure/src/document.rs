use gpui_kit::SharedString;
use mw_crystal::sample::{self, SampleKey};
use mw_crystal::{Derived, Lattice, Structure};

/// A distance between two atoms, as shown in the measurements panel and as
/// a label in the viewport.
#[derive(Clone, Debug)]
pub(crate) struct Measurement {
    pub label: SharedString,
    /// Cartesian Å.
    pub from: [f64; 3],
    pub to: [f64; 3],
}

impl Measurement {
    fn between(lattice: &Lattice, label: &str, from: [f64; 3], to: [f64; 3]) -> Self {
        Self {
            label: label.to_string().into(),
            from: lattice.to_cartesian(from),
            to: lattice.to_cartesian(to),
        }
    }

    pub fn distance(&self) -> f64 {
        self.from
            .iter()
            .zip(self.to)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            .sqrt()
    }

    pub fn midpoint(&self) -> [f64; 3] {
        [0, 1, 2].map(|i| (self.from[i] + self.to[i]) / 2.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiagnosticLevel {
    Info,
    Warning,
}

#[derive(Clone, Debug)]
pub(crate) struct Diagnostic {
    pub level: DiagnosticLevel,
    pub message: SharedString,
}

/// One open structure and everything shown about it.
pub struct StructureDocument {
    pub(crate) project_name: SharedString,
    pub(crate) name: SharedString,
    pub(crate) structure: Structure,
    pub(crate) derived: Derived,
    pub(crate) measurements: Vec<Measurement>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl StructureDocument {
    /// Mock: builds the document from a built-in sample instead of a file,
    /// with hand-written measurements and import diagnostics.
    pub fn from_sample(
        project_name: impl Into<SharedString>,
        name: impl Into<SharedString>,
        key: SampleKey,
    ) -> Self {
        let sample = sample::load(key);
        let lattice = sample.structure.lattice;
        let (measurements, diagnostics) = match key {
            SampleKey::Rutile => (
                vec![Measurement::between(
                    &lattice,
                    "Ti1–O1",
                    [0.5, 0.5, 0.5],
                    [0.8048, 0.1952, 0.5],
                )],
                vec![Diagnostic {
                    level: DiagnosticLevel::Info,
                    message: tr!(
                        "mock.diagnostic.snapped",
                        site = "O1",
                        wyckoff = "4f",
                        shift = "0.0003"
                    ),
                }],
            ),
            SampleKey::StrontiumTitanate => (
                vec![Measurement::between(
                    &lattice,
                    "Ti1–O1",
                    [0.5, 0.5, 0.5],
                    [0.5, 0.5, 0.0],
                )],
                Vec::new(),
            ),
            SampleKey::RutileSupercell => (
                Vec::new(),
                vec![Diagnostic {
                    level: DiagnosticLevel::Warning,
                    message: tr!("mock.diagnostic.supercell"),
                }],
            ),
        };
        Self {
            project_name: project_name.into(),
            name: name.into(),
            structure: sample.structure,
            derived: sample.derived,
            measurements,
            diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rutile_apical_bond_length() {
        let document = StructureDocument::from_sample("p", "rutile", SampleKey::Rutile);
        let distance = document.measurements[0].distance();
        assert!((distance - 1.980).abs() < 1e-3, "{distance}");
    }

    #[test]
    fn strontium_titanate_ti_o_is_half_the_cell() {
        let document = StructureDocument::from_sample("p", "SrTiO3", SampleKey::StrontiumTitanate);
        assert!((document.measurements[0].distance() - 3.905 / 2.0).abs() < 1e-9);
    }
}
