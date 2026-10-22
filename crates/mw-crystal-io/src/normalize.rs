//! The normalization step of the import (design §9.1, step 3): sites near
//! a special position move onto it, rows at one position become one site
//! with mixed occupancy, and rows that repeat a site are dropped. Every
//! change is reported with the line of the row it concerns.

use std::collections::{HashMap, HashSet};

use mw_crystal::{SiteId, Structure};

use crate::diagnostic::{Diagnostic, DiagnosticKind};

/// How far an occupancy sum may exceed 1 before it is reported;
/// refinements often end slightly above.
pub(crate) const OCCUPANCY_TOLERANCE: f64 = 0.01;

/// Å. Moves smaller than this are not reported: they are below the
/// precision of coordinates written with four or five decimals, such as
/// 0.33333 for 1/3.
const REPORTED_DISPLACEMENT: f64 = 1e-4;

/// Moves every site within the position tolerance of a special position
/// onto it: to the average of its images under the operations that leave
/// it in place (design §7.6).
pub(crate) fn snap_sites(
    structure: &mut Structure,
    lines: &HashMap<SiteId, u32>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for i in 0..structure.sites.len() {
        let site = &structure.sites[i];
        let special = mw_crystal::special_position(structure, site.position);
        if special.order == 1 || special.displacement == 0.0 {
            continue;
        }
        if special.displacement >= REPORTED_DISPLACEMENT {
            diagnostics.push(Diagnostic::new(
                DiagnosticKind::SnappedToSpecialPosition {
                    label: site.label().to_string(),
                    form: special.form.to_string(),
                    displacement: special.displacement,
                },
                lines.get(&site.id).copied(),
            ));
        }
        structure.sites[i].position = special.position;
    }
}

/// A site counts as fully occupied, rather than one of several partial
/// occupants, from this sum on.
fn is_full(occupancy: f64) -> bool {
    occupancy >= 1.0 - OCCUPANCY_TOLERANCE
}

/// Resolves sites that share a position (within the position tolerance,
/// after snapping), in file order:
///
/// - different disorder groups: alternatives, kept as they are;
/// - both fully occupied with the same species: the later row repeats the
///   earlier one and is dropped;
/// - both fully occupied with different species: a conflict, kept and
///   reported;
/// - otherwise mixed occupancy: the later row's occupants join the earlier
///   site.
pub(crate) fn merge_sites(
    structure: &mut Structure,
    lines: &HashMap<SiteId, u32>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let orbits = mw_crystal::orbits(structure);
    let contacts = mw_crystal::close_contacts(structure, &orbits, structure.tolerances.position);
    // Dropped or merged site → the site that took its place.
    let mut replaced: HashMap<SiteId, SiteId> = HashMap::new();
    let mut decided: HashSet<(SiteId, SiteId)> = HashSet::new();
    let mut merged_into: Vec<SiteId> = Vec::new();
    let resolve = |replaced: &HashMap<SiteId, SiteId>, mut id: SiteId| {
        while let Some(next) = replaced.get(&id) {
            id = *next;
        }
        id
    };
    for contact in contacts {
        let (a, b) = (resolve(&replaced, contact.a), resolve(&replaced, contact.b));
        // Keep file order: the earlier site stays.
        let (a, b) = (a.min(b), a.max(b));
        if a == b || !decided.insert((a, b)) {
            continue;
        }
        let (Some(site_a), Some(site_b)) = (structure.site(a), structure.site(b)) else {
            continue;
        };
        let (label_a, label_b) = (site_a.label().to_string(), site_b.label().to_string());
        let line = lines.get(&b).copied();
        let occupancy = |id: SiteId| {
            structure.site(id).map_or(0.0, |site| {
                site.occupants.iter().map(|o| o.occupancy.value()).sum()
            })
        };
        let species = |id: SiteId| {
            structure.site(id).map_or_else(Vec::new, |site| {
                site.occupants
                    .iter()
                    .map(|o| (o.species.element.clone(), o.occupancy.value()))
                    .collect::<Vec<_>>()
            })
        };
        if site_a.disorder != site_b.disorder {
            diagnostics.push(Diagnostic::new(
                DiagnosticKind::SharedPosition {
                    a: label_a,
                    b: label_b,
                },
                line,
            ));
            continue;
        }
        if is_full(occupancy(a)) && is_full(occupancy(b)) {
            if species(a) != species(b) {
                diagnostics.push(Diagnostic::new(
                    DiagnosticKind::OverlappingSites {
                        a: label_a,
                        b: label_b,
                    },
                    line,
                ));
                continue;
            }
            diagnostics.push(Diagnostic::new(
                DiagnosticKind::RedundantSite {
                    label: label_b,
                    duplicate_of: label_a,
                },
                line,
            ));
        } else {
            let occupants = structure.site(b).map(|s| s.occupants.clone());
            if let Some(site) = structure.sites.iter_mut().find(|s| s.id == a) {
                site.occupants.extend(occupants.into_iter().flatten());
            }
            diagnostics.push(Diagnostic::new(
                DiagnosticKind::MergedOccupancy {
                    label: label_b,
                    into: label_a,
                },
                line,
            ));
            if !merged_into.contains(&a) {
                merged_into.push(a);
            }
        }
        structure.sites.retain(|site| site.id != b);
        replaced.insert(b, a);
    }
    for id in merged_into {
        let Some(site) = structure.site(id) else {
            continue;
        };
        let sum: f64 = site.occupants.iter().map(|o| o.occupancy.value()).sum();
        if sum > 1.0 + OCCUPANCY_TOLERANCE {
            diagnostics.push(Diagnostic::new(
                DiagnosticKind::OccupancySumAboveOne {
                    label: site.label().to_string(),
                    sum,
                },
                lines.get(&id).copied(),
            ));
        }
    }
}
