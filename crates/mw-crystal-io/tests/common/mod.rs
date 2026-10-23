//! The CIF round trip shared by `export.rs` and `cod_roundtrip.rs`: write a
//! structure, read it back, compare.

// Each test file uses its own part of this module.
#![allow(dead_code)]

use mw_crystal::{Adp, CoordinateConstraint, Measured, Structure, SymmetryAnalysis};
use mw_crystal_io::{ImportedBlock, Severity, cif};

/// What a round trip produced.
pub struct RoundTrip {
    pub text: String,
    pub block: ImportedBlock,
}

/// Why a round trip failed, as a short category for statistics and a
/// detail for the message.
#[derive(Debug)]
pub struct Mismatch {
    pub category: &'static str,
    pub detail: String,
}

fn mismatch(category: &'static str, detail: impl Into<String>) -> Mismatch {
    Mismatch {
        category,
        detail: detail.into(),
    }
}

/// Writes the structure of `block` as CIF, reads the text back and checks
/// that it gives the same structure, without new errors or warnings, and
/// that writing it again gives the same text.
pub fn round_trip(block: &ImportedBlock, structure: &Structure) -> Result<RoundTrip, Mismatch> {
    let written = cif::write(structure, &block.write_options(structure));
    if !written.dropped.is_empty() {
        return Err(mismatch("dropped", written.dropped.join(", ")));
    }
    let report = cif::read_str(&written.text);
    let [again] = &report.blocks[..] else {
        return Err(mismatch(
            "blocks",
            format!("{} blocks read back", report.blocks.len()),
        ));
    };
    let new_problems: Vec<_> = report
        .all_diagnostics()
        .filter(|d| match d.severity() {
            Severity::Error => true,
            Severity::Warning => !block.diagnostics.iter().any(|o| o.code() == d.code()),
            Severity::Info => false,
        })
        .map(|d| format!("{} (line {:?})", d.code(), d.line()))
        .collect();
    if !new_problems.is_empty() {
        return Err(mismatch("diagnostics", new_problems.join(", ")));
    }
    let Some(read_back) = &again.structure else {
        return Err(mismatch("no structure", ""));
    };
    compare(structure, read_back)?;
    if again.fields != block.fields {
        let (before, after) = (
            format!("{:#?}", block.fields),
            format!("{:#?}", again.fields),
        );
        let line = before
            .lines()
            .zip(after.lines())
            .find(|(a, b)| a != b)
            .map_or_else(String::new, |(a, b)| {
                format!("{} became {}", a.trim(), b.trim())
            });
        return Err(mismatch("kept fields", line));
    }
    let rewritten = cif::write(read_back, &again.write_options(read_back));
    if rewritten.text != written.text {
        let line = written
            .text
            .lines()
            .zip(rewritten.text.lines())
            .find(|(a, b)| a != b)
            .map_or_else(String::new, |(a, b)| format!("{a:?} became {b:?}"));
        return Err(mismatch("not idempotent", line));
    }
    Ok(RoundTrip {
        text: written.text,
        block: again.clone(),
    })
}

/// Whether two measured values agree: exactly up to reading decimals back,
/// or within the rounding that writing them with their uncertainty allows.
fn close(a: Measured, b: Measured) -> bool {
    let slack = a.su().map_or(0.0, |su| su / 2.0);
    let agree = |x: f64, y: f64, slack: f64| (x - y).abs() <= 1e-9 * x.abs().max(1.0) + slack;
    agree(a.value(), b.value(), slack)
        && match (a.su(), b.su()) {
            (None, None) => true,
            (Some(x), Some(y)) => agree(x, y, x / 2.0),
            _ => false,
        }
}

/// Compares two structures within the precision CIF text keeps. Sites are
/// compared in order, not by id: dropped rows shift the ids.
pub fn compare(a: &Structure, b: &Structure) -> Result<(), Mismatch> {
    let (pa, pb) = (a.lattice.params(), b.lattice.params());
    let cell = [
        (pa.a, pb.a),
        (pa.b, pb.b),
        (pa.c, pb.c),
        (pa.alpha, pb.alpha),
        (pa.beta, pb.beta),
        (pa.gamma, pb.gamma),
    ];
    if let Some((x, y)) = cell.iter().find(|(x, y)| !close(*x, *y)) {
        return Err(mismatch("cell", format!("{x:?} became {y:?}")));
    }
    if a.symmetry != b.symmetry {
        return Err(mismatch("operations", "the operation lists differ"));
    }
    if a.sites.len() != b.sites.len() {
        return Err(mismatch(
            "sites",
            format!("{} sites became {}", a.sites.len(), b.sites.len()),
        ));
    }
    let analysis = SymmetryAnalysis::compute(a, &mw_crystal::orbits(a));
    for (sa, sb) in a.sites.iter().zip(&b.sites) {
        let label = sa.label();
        let constraints = analysis
            .sites
            .iter()
            .find(|(id, _)| *id == sa.id)
            .map(|(_, s)| s.coordinates);
        // A coordinate fixed by symmetry is written without its s.u.
        let su = |axis: usize| {
            let fixed = constraints.is_some_and(|c| c[axis] == CoordinateConstraint::Fixed);
            sa.position_su[axis].filter(|_| !fixed)
        };
        for axis in 0..3 {
            // One with an s.u. is rounded to it, e.g. an average of three
            // coordinates that snapping made equal; one that follows such a
            // coordinate is rounded like it.
            let rounding = match constraints.map(|c| c[axis]) {
                Some(CoordinateConstraint::Follows(pivot)) if su(axis).is_none() => {
                    su(pivot as usize)
                }
                _ => su(axis),
            };
            let slack = rounding.map_or(0.0, |su| su / 2.0);
            if (sa.position[axis] - sb.position[axis]).abs() > 1e-8 + slack {
                return Err(mismatch(
                    "position",
                    format!("{label}: {:?} became {:?}", sa.position, sb.position),
                ));
            }
            let agree = match (su(axis), sb.position_su[axis]) {
                (None, None) => true,
                (Some(x), Some(y)) => close(
                    Measured::with_su(sa.position[axis], x),
                    Measured::with_su(sb.position[axis], y),
                ),
                _ => false,
            };
            if !agree {
                return Err(mismatch(
                    "position s.u.",
                    format!("{label}: {:?} became {:?}", sa.position_su, sb.position_su),
                ));
            }
        }
        if sa.disorder != sb.disorder {
            return Err(mismatch("disorder", label.to_string()));
        }
        if sa.occupants.len() != sb.occupants.len() {
            return Err(mismatch("occupants", label.to_string()));
        }
        for (oa, ob) in sa.occupants.iter().zip(&sb.occupants) {
            if oa.label != ob.label || oa.species != ob.species {
                return Err(mismatch(
                    "species",
                    format!(
                        "{} {:?} became {} {:?}",
                        oa.label, oa.species, ob.label, ob.species
                    ),
                ));
            }
            if !close(oa.occupancy, ob.occupancy) {
                return Err(mismatch("occupancy", oa.label.clone()));
            }
            let adp = match (&oa.adp, &ob.adp) {
                (None, None) => true,
                (Some(Adp::Isotropic(x)), Some(Adp::Isotropic(y))) => close(*x, *y),
                (Some(Adp::Anisotropic(x)), Some(Adp::Anisotropic(y))) => {
                    x.iter().zip(y).all(|(x, y)| close(*x, *y))
                }
                _ => false,
            };
            if !adp {
                return Err(mismatch(
                    "adp",
                    format!("{}: {:?} became {:?}", oa.label, oa.adp, ob.adp),
                ));
            }
        }
    }
    if a.tolerances != b.tolerances {
        return Err(mismatch("tolerances", ""));
    }
    Ok(())
}
