//! The atoms around an atom (design §5.3, the neighbor list), for questions
//! such as an atom's coordination.

use crate::{AtomRef, Derived, Lattice};

/// An atom near another.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Neighbor {
    pub atom: AtomRef,
    /// Å.
    pub distance: f64,
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

impl Derived {
    /// The atoms within `cutoff` Å of `atom`, nearest first, without the atom
    /// itself; `None` if there is no such atom.
    pub fn neighbors(
        &self,
        lattice: &Lattice,
        atom: AtomRef,
        cutoff: f64,
    ) -> Option<Vec<Neighbor>> {
        let center = self.fractional(atom)?;
        let params = lattice.params();
        let matrix = params.matrix();
        let volume = params.volume();
        if !(volume > 0.0 && cutoff > 0.0) {
            return Some(Vec::new());
        }
        // How many cells away along each axis a neighbor can be: the cutoff
        // over the spacing of the lattice planes, as in the bond search.
        let reach = [(1, 2), (2, 0), (0, 1)]
            .map(|(i, j)| cutoff * norm(cross(matrix[i], matrix[j])) / volume);
        let cartesian = |f: [f64; 3]| {
            [0, 1, 2].map(|i| f[0] * matrix[0][i] + f[1] * matrix[1][i] + f[2] * matrix[2][i])
        };
        let origin = cartesian(center);
        let mut found = Vec::new();
        for orbit in &self.orbits {
            for (member, position) in orbit.positions.iter().enumerate() {
                let range = |axis: usize| {
                    let low = (center[axis] - reach[axis] - position[axis]).floor() as i32;
                    let high = (center[axis] + reach[axis] - position[axis]).ceil() as i32;
                    low..=high
                };
                for x in range(0) {
                    for y in range(1) {
                        for z in range(2) {
                            let cell = [x, y, z];
                            let candidate = AtomRef::new(orbit.site, member as u16, cell);
                            if candidate == atom {
                                continue;
                            }
                            let p = cartesian([0, 1, 2].map(|i| position[i] + f64::from(cell[i])));
                            let distance = norm([0, 1, 2].map(|i| p[i] - origin[i]));
                            if distance <= cutoff {
                                found.push(Neighbor {
                                    atom: candidate,
                                    distance,
                                });
                            }
                        }
                    }
                }
            }
        }
        found.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then_with(|| a.atom.cmp(&b.atom))
        });
        Some(found)
    }
}

#[cfg(test)]
mod tests {
    use crate::AtomRef;
    use crate::sample::{self, SampleKey};

    #[test]
    fn octahedra() {
        let sample = sample::load(SampleKey::StrontiumTitanate);
        let (lattice, derived) = (&sample.structure.lattice, &sample.derived);
        let ti = AtomRef::new(sample.structure.sites[1].id, 0, [0; 3]);
        let o = sample.structure.sites[2].id;
        let near = derived.neighbors(lattice, ti, 2.0).unwrap();
        assert_eq!(near.len(), 6);
        assert!(near.iter().all(|n| n.atom.site == o));
        assert!(near.iter().all(|n| (n.distance - 3.905 / 2.0).abs() < 1e-9));
        // Then the eight Sr at the cube corners.
        let wider = derived.neighbors(lattice, ti, 3.5).unwrap();
        assert_eq!(wider.len(), 14);
        assert!((wider[13].distance - 3.905 * 3f64.sqrt() / 2.0).abs() < 1e-9);

        // Around an atom in another cell, the same shell.
        let image = ti.translated([2, -1, 0]);
        assert_eq!(derived.neighbors(lattice, image, 2.0).unwrap().len(), 6);

        let rutile = sample::load(SampleKey::Rutile);
        let ti = AtomRef::new(rutile.structure.sites[0].id, 0, [0; 3]);
        let near = rutile
            .derived
            .neighbors(&rutile.structure.lattice, ti, 2.1)
            .unwrap();
        assert_eq!(near.len(), 6, "{near:?}");
        assert!(
            derived
                .neighbors(lattice, AtomRef::new(o, 7, [0; 3]), 2.0)
                .is_none()
        );
    }
}
