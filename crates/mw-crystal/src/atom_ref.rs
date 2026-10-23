//! References to single atoms of the crystal (design §5.1), and the
//! measurements made between them.

use serde::{Deserialize, Serialize};

use crate::{Derived, Lattice, SiteId};

/// One atom of the infinite crystal: member `member` of a site's orbit,
/// translated by `cell` lattice vectors. Selections, measurements and bonds
/// refer to atoms this way.
///
/// Member numbers stay valid as long as the operation set and the site's
/// site symmetry do not change (design §5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AtomRef {
    pub site: SiteId,
    pub member: u16,
    pub cell: [i32; 3],
}

impl AtomRef {
    pub fn new(site: SiteId, member: u16, cell: [i32; 3]) -> Self {
        Self { site, member, cell }
    }

    /// The same orbit member, translated by `offset` more cells.
    pub fn translated(self, offset: [i32; 3]) -> Self {
        Self {
            cell: [0, 1, 2].map(|i| self.cell[i] + offset[i]),
            ..self
        }
    }
}

impl Derived {
    /// Fractional coordinates of an atom, or `None` if the structure has no
    /// such site or orbit member.
    pub fn fractional(&self, atom: AtomRef) -> Option<[f64; 3]> {
        let position = self.orbit(atom.site)?.positions.get(atom.member as usize)?;
        Some([0, 1, 2].map(|i| position[i] + f64::from(atom.cell[i])))
    }

    /// Cartesian coordinates of an atom in Å.
    pub fn cartesian(&self, lattice: &Lattice, atom: AtomRef) -> Option<[f64; 3]> {
        self.fractional(atom).map(|p| lattice.to_cartesian(p))
    }
}

/// A distance or angle between atoms (design §6, document annotations).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measurement {
    Distance([AtomRef; 2]),
    /// The angle at the middle atom.
    Angle([AtomRef; 3]),
}

impl Measurement {
    pub fn atoms(&self) -> &[AtomRef] {
        match self {
            Self::Distance(atoms) => atoms,
            Self::Angle(atoms) => atoms,
        }
    }

    pub(crate) fn atoms_mut(&mut self) -> &mut [AtomRef] {
        match self {
            Self::Distance(atoms) => atoms,
            Self::Angle(atoms) => atoms,
        }
    }

    /// Å for a distance, degrees for an angle. `None` once an atom it refers
    /// to no longer exists; the measurement is then stale (design §7.11).
    pub fn value(&self, lattice: &Lattice, derived: &Derived) -> Option<f64> {
        let positions = self
            .atoms()
            .iter()
            .map(|atom| derived.cartesian(lattice, *atom))
            .collect::<Option<Vec<_>>>()?;
        Some(match positions[..] {
            [a, b] => distance(a, b),
            [a, vertex, c] => angle(a, vertex, c),
            _ => unreachable!("measurements have two or three atoms"),
        })
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Distance between two Cartesian points.
pub fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = sub(a, b);
    dot(d, d).sqrt()
}

/// The angle a–vertex–c in degrees; 0 when an arm has no length.
pub fn angle(a: [f64; 3], vertex: [f64; 3], c: [f64; 3]) -> f64 {
    let (u, v) = (sub(a, vertex), sub(c, vertex));
    let lengths = dot(u, u).sqrt() * dot(v, v).sqrt();
    if lengths == 0.0 {
        return 0.0;
    }
    (dot(u, v) / lengths).clamp(-1.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{self, SampleKey};

    #[test]
    fn resolves_members_and_cells() {
        let sample = sample::load(SampleKey::StrontiumTitanate);
        let lattice = sample.structure.lattice;
        let derived = &sample.derived;
        let ti = derived.orbits[1].site;
        let o = derived.orbits[2].site;

        let center = AtomRef::new(ti, 0, [0, 0, 0]);
        assert_eq!(derived.fractional(center), Some([0.5, 0.5, 0.5]));
        let shifted = center.translated([1, 0, -1]);
        assert_eq!(derived.fractional(shifted), Some([1.5, 0.5, -0.5]));
        assert_eq!(derived.fractional(AtomRef::new(ti, 1, [0, 0, 0])), None);
        assert_eq!(
            derived.fractional(AtomRef::new(SiteId(99), 0, [0; 3])),
            None
        );

        let ti_o = Measurement::Distance([center, AtomRef::new(o, 0, [0, 0, 0])]);
        let length = ti_o.value(&lattice, derived).unwrap();
        assert!((length - 3.905 / 2.0).abs() < 1e-9, "{length}");
    }

    #[test]
    fn angles_between_octahedron_corners() {
        let o = |p: [f64; 3]| p;
        let center = [0.0, 0.0, 0.0];
        assert!((angle(o([1.0, 0.0, 0.0]), center, o([0.0, 2.0, 0.0])) - 90.0).abs() < 1e-12);
        assert!((angle(o([1.0, 0.0, 0.0]), center, o([-3.0, 0.0, 0.0])) - 180.0).abs() < 1e-12);
        assert_eq!(angle(center, center, o([1.0, 0.0, 0.0])), 0.0);
    }

    #[test]
    fn stale_measurements_have_no_value() {
        let sample = sample::load(SampleKey::Rutile);
        let missing = AtomRef::new(SiteId(42), 0, [0; 3]);
        let present = AtomRef::new(sample.derived.orbits[0].site, 0, [0; 3]);
        let m = Measurement::Angle([present, missing, present]);
        assert_eq!(m.value(&sample.structure.lattice, &sample.derived), None);
    }

    #[test]
    fn the_orientation_moves_atoms_but_changes_no_distance() {
        let rutile = sample::load(SampleKey::Rutile).structure;
        let standard = Derived::compute(&rutile);
        let ti = standard.orbits[0].site;
        let o = standard.orbits[1].site;
        let atoms = [
            AtomRef::new(ti, 0, [0; 3]),
            AtomRef::new(o, 0, [0; 3]),
            AtomRef::new(o, 1, [0, 0, 1]),
        ];
        let distance = Measurement::Distance([atoms[0], atoms[1]]);
        let angle = Measurement::Angle(atoms);
        let (s, c) = 0.7f64.sin_cos();
        for rows in [
            [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
            // Left-handed: a mirror in the xz plane.
            [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]],
        ] {
            let mut turned = rutile.clone();
            turned.lattice = Lattice::with_orientation(
                *rutile.lattice.params(),
                crate::Orientation::new(crate::Rotation::new(rows).unwrap()),
            );
            let derived = Derived::compute(&turned);
            // The O atom moved in Cartesian space...
            let before = standard.cartesian(&rutile.lattice, atoms[1]).unwrap();
            let after = derived.cartesian(&turned.lattice, atoms[1]).unwrap();
            assert!((0..3).any(|i| (before[i] - after[i]).abs() > 0.1));
            // ...but distances, angles, bonds and neighbors stay.
            for m in [distance, angle] {
                let (a, b) = (
                    m.value(&rutile.lattice, &standard).unwrap(),
                    m.value(&turned.lattice, &derived).unwrap(),
                );
                assert!((a - b).abs() < 1e-9, "{m:?}: {a} vs {b}");
            }
            // The same bonds; their order within one first atom follows
            // the Cartesian search grid.
            let sorted = |derived: &Derived| {
                let mut bonds: Vec<_> =
                    derived.bonds.iter().map(|b| (b.a, b.b, b.length)).collect();
                bonds.sort_by_key(|(a, b, _)| (*a, *b));
                bonds
            };
            let (a, b) = (sorted(&derived), sorted(&standard));
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(&b) {
                assert_eq!((a.0, a.1), (b.0, b.1));
                assert!((a.2 - b.2).abs() < 1e-9);
            }
            let neighbors = |lattice: &Lattice, derived: &Derived| {
                derived.neighbors(lattice, atoms[0], 4.0).unwrap()
            };
            let (a, b) = (
                neighbors(&rutile.lattice, &standard),
                neighbors(&turned.lattice, &derived),
            );
            assert_eq!(a.len(), b.len());
            assert!(
                a.iter()
                    .zip(&b)
                    .all(|(a, b)| (a.distance - b.distance).abs() < 1e-9)
            );
        }
    }
}
