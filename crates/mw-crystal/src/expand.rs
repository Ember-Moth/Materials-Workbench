//! The minimal expansion of the asymmetric unit (design §5.1): every site's
//! equivalent atoms in the unit cell, the atoms drawn for one cell, and
//! atoms that come too close to each other.
//!
//! Distances use the minimum image found by rounding fractional differences.
//! That is exact for distances below half the smallest spacing between
//! lattice planes, which holds for the tolerances used here in any real cell.

use crate::{AtomRef, DisplayAtom, Site, SiteId, Structure};

/// One site's equivalent atoms in the unit cell.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Orbit {
    pub site: SiteId,
    /// Fractional coordinates in [0, 1), in the order of the operations
    /// that first produced them; the first is the site itself.
    pub positions: Vec<[f64; 3]>,
    /// Å. Distance from the site to its nearest equivalent atom, if it has
    /// any in the cell.
    pub nearest_image: Option<f64>,
}

/// Two different sites whose atoms come close to each other.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Contact {
    pub a: SiteId,
    pub b: SiteId,
    /// Å, the shortest distance between an atom of `a` and one of `b`.
    pub distance: f64,
}

/// Fractional-to-Cartesian conversion with the matrix computed once.
struct Metric([[f64; 3]; 3]);

impl Metric {
    fn new(structure: &Structure) -> Self {
        Self(structure.lattice.params().matrix())
    }

    fn length(&self, fractional: [f64; 3]) -> f64 {
        let mut r = [0.0; 3];
        for (axis, basis) in self.0.iter().enumerate() {
            for (i, component) in basis.iter().enumerate() {
                r[i] += fractional[axis] * component;
            }
        }
        r.iter().map(|v| v * v).sum::<f64>().sqrt()
    }

    /// Distance in Å between the nearest images of two fractional positions.
    fn distance(&self, a: [f64; 3], b: [f64; 3]) -> f64 {
        self.length([0, 1, 2].map(|i| {
            let d = a[i] - b[i];
            d - d.round()
        }))
    }
}

/// Wraps fractional coordinates into [0, 1).
pub(crate) fn wrap(x: [f64; 3]) -> [f64; 3] {
    x.map(|v| {
        let w = v - v.floor();
        // A tiny negative value wraps to exactly 1.0 in floating point.
        if w >= 1.0 { 0.0 } else { w }
    })
}

/// One site's equivalent atoms in the cell, each with the index of the
/// operation that first produced it: the members of its orbit, in order.
pub(crate) fn members(structure: &Structure, site: &Site) -> Vec<(usize, [f64; 3])> {
    let metric = Metric::new(structure);
    let tolerance = structure.tolerances.position;
    let mut members: Vec<(usize, [f64; 3])> = Vec::new();
    for (index, op) in structure.symmetry.operations().iter().enumerate() {
        let image = wrap(op.apply(site.position));
        if members
            .iter()
            .all(|(_, p)| metric.distance(*p, image) >= tolerance)
        {
            members.push((index, image));
        }
    }
    members
}

/// Applies every operation to every site, wraps the results into the cell
/// and keeps one atom per position within the position tolerance.
pub fn orbits(structure: &Structure) -> Vec<Orbit> {
    let metric = Metric::new(structure);
    structure
        .sites
        .iter()
        .map(|site| {
            let positions: Vec<[f64; 3]> = members(structure, site)
                .into_iter()
                .map(|(_, position)| position)
                .collect();
            let nearest_image = positions
                .iter()
                .skip(1)
                .map(|p| metric.distance(positions[0], *p))
                .min_by(f64::total_cmp);
            Orbit {
                site: site.id,
                positions,
                nearest_image,
            }
        })
        .collect()
}

/// The atoms drawn for one unit cell: every orbit member, plus a copy on the
/// opposite face for members that lie on a face, edge or corner.
///
/// Mock: stands in for the view layer's display range (design §5.3, v0.3).
pub fn display_atoms(structure: &Structure, orbits: &[Orbit]) -> Vec<DisplayAtom> {
    let params = structure.lattice.params();
    let lengths = [params.a.value(), params.b.value(), params.c.value()];
    let tolerance = lengths.map(|l| structure.tolerances.position / l);
    let mut atoms = Vec::new();
    for orbit in orbits {
        let Some(site) = structure.site(orbit.site) else {
            continue;
        };
        for (member, position) in orbit.positions.iter().enumerate() {
            let images = |axis: usize| {
                let v = position[axis];
                if v < tolerance[axis] {
                    vec![v, v + 1.0]
                } else if v > 1.0 - tolerance[axis] {
                    vec![v - 1.0, v]
                } else {
                    vec![v]
                }
            };
            for x in images(0) {
                for y in images(1) {
                    for z in images(2) {
                        let cell = [x, y, z].map(|v: f64| v.floor() as i32);
                        atoms.push(DisplayAtom {
                            atom: AtomRef::new(site.id, member as u16, cell),
                            element: site.element().to_string(),
                            position: structure.lattice.to_cartesian([x, y, z]),
                        });
                    }
                }
            }
        }
    }
    atoms
}

/// Pairs of different sites with atoms closer than `max_distance` Å, each
/// pair once, in site order.
pub fn close_contacts(structure: &Structure, orbits: &[Orbit], max_distance: f64) -> Vec<Contact> {
    let metric = Metric::new(structure);
    let mut contacts = Vec::new();
    for (i, a) in orbits.iter().enumerate() {
        for b in &orbits[i + 1..] {
            // Every pair of atoms maps onto a pair that contains b's own
            // position, so comparing a's members with it is enough.
            let distance = a
                .positions
                .iter()
                .map(|p| metric.distance(*p, b.positions[0]))
                .fold(f64::INFINITY, f64::min);
            if distance < max_distance {
                contacts.push(Contact {
                    a: a.site,
                    b: b.site,
                    distance,
                });
            }
        }
    }
    contacts
}

/// Other sites with an atom closer than `max_distance` Å to an atom of
/// `site`, in site order.
pub(crate) fn contacts_with(
    structure: &Structure,
    orbits: &[Orbit],
    site: SiteId,
    max_distance: f64,
) -> Vec<Contact> {
    let metric = Metric::new(structure);
    let Some(own) = orbits.iter().find(|orbit| orbit.site == site) else {
        return Vec::new();
    };
    orbits
        .iter()
        .filter(|other| other.site != site)
        .filter_map(|other| {
            // As in `close_contacts`, the other site's own position suffices.
            let distance = own
                .positions
                .iter()
                .map(|p| metric.distance(*p, other.positions[0]))
                .fold(f64::INFINITY, f64::min);
            (distance < max_distance).then_some(Contact {
                a: site,
                b: other.site,
                distance,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellParams, Lattice, Site, SymOp, Symmetry};

    fn structure(generators: &[&str], sites: Vec<Site>) -> Structure {
        let generators: Vec<SymOp> = generators
            .iter()
            .map(|g| SymOp::parse(g).unwrap())
            .collect();
        Structure::new(
            Lattice::new(CellParams::new(4.0, 4.0, 4.0, 90.0, 90.0, 90.0)),
            Symmetry::generate(&generators).unwrap(),
            sites,
        )
    }

    /// Pm-3m.
    const CUBIC: [&str; 3] = ["-y,x,z", "z,x,y", "-x,-y,-z"];

    #[test]
    fn orbit_sizes_follow_site_symmetry() {
        let s = structure(
            &CUBIC,
            vec![
                Site::single(1, "A", "Sr", [0.0, 0.0, 0.0]),
                Site::single(2, "B", "O", [0.0, 0.5, 0.5]),
                Site::single(3, "C", "O", [0.1, 0.2, 0.3]),
                // Equivalent to the origin; wraps instead of adding an atom.
                Site::single(4, "D", "Sr", [1.0, -1.0, 2.0]),
            ],
        );
        let sizes: Vec<_> = orbits(&s).iter().map(|o| o.positions.len()).collect();
        assert_eq!(sizes, [1, 3, 48, 1]);
    }

    #[test]
    fn first_member_is_the_wrapped_site() {
        let s = structure(&CUBIC, vec![Site::single(1, "C", "O", [-0.1, 1.2, 0.3])]);
        let first = orbits(&s)[0].positions[0];
        for (a, b) in first.iter().zip([0.9, 0.2, 0.3]) {
            assert!((a - b).abs() < 1e-12);
        }
    }

    #[test]
    fn positions_within_tolerance_merge() {
        // 0.001 Å off the mirror plane: one atom, not two.
        let s = structure(
            &["x,y,-z"],
            vec![Site::single(1, "A", "O", [0.1, 0.2, 0.00025])],
        );
        let orbit = &orbits(&s)[0];
        assert_eq!(orbit.positions.len(), 1);
        assert_eq!(orbit.nearest_image, None);

        // 0.04 Å off: two atoms 0.08 Å apart, reported as close images.
        let s = structure(
            &["x,y,-z"],
            vec![Site::single(1, "A", "O", [0.1, 0.2, 0.01])],
        );
        let orbit = &orbits(&s)[0];
        assert_eq!(orbit.positions.len(), 2);
        assert!((orbit.nearest_image.unwrap() - 0.08).abs() < 1e-9);
    }

    #[test]
    fn face_atoms_are_repeated_on_the_opposite_face() {
        let s = structure(
            &CUBIC,
            vec![
                Site::single(1, "Sr1", "Sr", [0.0, 0.0, 0.0]),
                Site::single(2, "Ti1", "Ti", [0.5, 0.5, 0.5]),
                Site::single(3, "O1", "O", [0.0, 0.5, 0.5]),
            ],
        );
        let atoms = display_atoms(&s, &orbits(&s));
        assert_eq!(atoms.len(), 8 + 1 + 6);
        assert!(
            atoms
                .iter()
                .all(|a| a.position.iter().all(|v| (0.0..=4.0).contains(v)))
        );
    }

    #[test]
    fn finds_close_contacts_across_cell_edges() {
        let s = structure(
            &["x,y,z"],
            vec![
                Site::single(1, "A", "O", [0.01, 0.5, 0.5]),
                Site::single(2, "B", "O", [0.97, 0.5, 0.5]),
                Site::single(3, "C", "O", [0.5, 0.5, 0.5]),
            ],
        );
        let contacts = close_contacts(&s, &orbits(&s), 0.5);
        assert_eq!(contacts.len(), 1);
        assert_eq!((contacts[0].a, contacts[0].b), (SiteId(1), SiteId(2)));
        assert!((contacts[0].distance - 0.16).abs() < 1e-9);
    }
}
