//! Displacement ellipsoids (design §5.8): each atom's displacement
//! parameters in Cartesian Å², and the ellipsoid that holds the atom's
//! center with a given probability, which the "ellipsoids" display model
//! draws. The probability is view state.

use std::collections::HashMap;

use nalgebra::{Matrix3, SymmetricEigen};

use crate::{Adp, AtomRef, SiteId, Structure, expand};

/// The probability ORTEP drawings use unless they say otherwise.
pub const DEFAULT_PROBABILITY: f64 = 0.5;

/// How an atom's displacement is drawn (design §5.8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Displacement {
    /// Anisotropic parameters: the ellipsoid's semi-axes in Cartesian Å,
    /// longest first.
    Ellipsoid([[f64; 3]; 3]),
    /// An isotropic parameter: a sphere of this radius in Å.
    Sphere(f64),
    /// The atom has no displacement parameters.
    Missing,
    /// The parameters are not positive definite (U_iso ≤ 0 included), so
    /// no ellipsoid exists.
    NotPositiveDefinite,
}

/// The scale C of the ellipsoid `rᵀ·U⁻¹·r = C²` that holds the center of a
/// vibrating atom with `probability`: `P(χ²₃ ≤ C²) = p`, as ORTEP draws
/// them (C = 1.5382 at 50 %). The probability is clamped to (0, 1).
pub fn probability_scale(probability: f64) -> f64 {
    let p = probability.clamp(1e-9, 1.0 - 1e-9);
    // P(χ²₃ ≤ C²) = P(3/2, C²/2), which grows with C: bisect.
    let (mut low, mut high) = (0.0f64, 12.0f64);
    for _ in 0..100 {
        let middle = 0.5 * (low + high);
        if lower_gamma_3_2(0.5 * middle * middle) < p {
            low = middle;
        } else {
            high = middle;
        }
    }
    0.5 * (low + high)
}

/// The regularized lower incomplete gamma function P(3/2, x), by its
/// series `e⁻ˣ·x^a·Σ xⁿ/Γ(a + n + 1)`: every term is positive, so there is
/// no cancellation.
fn lower_gamma_3_2(x: f64) -> f64 {
    const A: f64 = 1.5;
    // Γ(5/2) = 3√π/4.
    let mut term = 1.0 / (0.75 * std::f64::consts::PI.sqrt());
    let mut sum = term;
    let mut n = 0.0;
    while term > 1e-17 * sum {
        term *= x / (A + n + 1.0);
        sum += term;
        n += 1.0;
    }
    (-x).exp() * x.powf(A) * sum
}

/// The displacement of every atom of the unit cell at one probability.
#[derive(Clone, Debug, Default)]
pub struct Displacements {
    members: HashMap<(SiteId, u16), Displacement>,
    missing: Vec<SiteId>,
    not_positive_definite: Vec<SiteId>,
}

impl Displacements {
    /// The ellipsoids of the structure's atoms at `probability`. A site
    /// with mixed occupancy takes the first occupant's parameters, as it
    /// takes its element (design §5.3). Each orbit member's comes from
    /// the site's through the rotation of the operation that made it.
    pub fn new(structure: &Structure, probability: f64) -> Self {
        let scale = probability_scale(probability);
        let basis = Matrix3::from(structure.lattice.matrix());
        let reciprocal = structure.lattice.params().reciprocal_lengths();
        let operations = structure.symmetry.operations();
        let mut displacements = Self::default();
        for site in &structure.sites {
            let adp = site.occupants.first().and_then(|o| o.adp);
            let positive = adp.is_some_and(|adp| is_positive_definite(&adp, &basis, &reciprocal));
            match adp {
                None => displacements.missing.push(site.id),
                Some(_) if !positive => displacements.not_positive_definite.push(site.id),
                Some(_) => {}
            }
            for (member, (op, _)) in expand::members(structure, site).into_iter().enumerate() {
                let displacement = match adp {
                    None => Displacement::Missing,
                    Some(_) if !positive => Displacement::NotPositiveDefinite,
                    Some(Adp::Isotropic(u)) => Displacement::Sphere(scale * u.value().sqrt()),
                    Some(Adp::Anisotropic(u)) => {
                        let rotation = operations[op].rotation();
                        let cartesian =
                            cartesian(&u.map(|m| m.value()), rotation, &basis, &reciprocal);
                        ellipsoid(cartesian, scale)
                    }
                };
                displacements
                    .members
                    .insert((site.id, member as u16), displacement);
            }
        }
        displacements
    }

    /// The displacement of an atom, in any cell; `Missing` for one the
    /// structure does not have.
    pub fn get(&self, atom: AtomRef) -> Displacement {
        self.members
            .get(&(atom.site, atom.member))
            .copied()
            .unwrap_or(Displacement::Missing)
    }

    /// The sites without displacement parameters, in order.
    pub fn missing(&self) -> &[SiteId] {
        &self.missing
    }

    /// The sites whose parameters are not positive definite, in order.
    pub fn not_positive_definite(&self) -> &[SiteId] {
        &self.not_positive_definite
    }
}

/// Whether a site's parameters describe an ellipsoid: U_iso > 0, or every
/// eigenvalue of U positive. Rotations keep the eigenvalues, so the
/// representative's answer holds for the whole orbit.
fn is_positive_definite(adp: &Adp, basis: &Matrix3<f64>, reciprocal: &[f64; 3]) -> bool {
    match adp {
        Adp::Isotropic(u) => u.value() > 0.0,
        Adp::Anisotropic(u) => {
            let identity = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
            let u = cartesian(&u.map(|m| m.value()), &identity, basis, reciprocal);
            u.iter().all(|v| v.is_finite())
                && SymmetricEigen::new(u)
                    .eigenvalues
                    .iter()
                    .all(|&lambda| lambda > 0.0)
        }
    }
}

/// U in Cartesian Å² of the atom an operation with `rotation` W makes from
/// a site with CIF parameters `u` (U11, U22, U33, U12, U13, U23):
/// `A·W·N·U·N·Wᵀ·Aᵀ` with `N = diag(a*, b*, c*)` and A the lattice's basis
/// in its orientation (design §4.2).
fn cartesian(
    u: &[f64; 6],
    rotation: &[[i32; 3]; 3],
    basis: &Matrix3<f64>,
    reciprocal: &[f64; 3],
) -> Matrix3<f64> {
    let [u11, u22, u33, u12, u13, u23] = *u;
    let cif = Matrix3::new(u11, u12, u13, u12, u22, u23, u13, u23, u33);
    let n = Matrix3::from_diagonal(&nalgebra::Vector3::from(*reciprocal));
    let w = Matrix3::from_fn(|i, j| f64::from(rotation[i][j]));
    let fractional = w * n * cif * n * w.transpose();
    basis * fractional * basis.transpose()
}

/// The ellipsoid of a positive definite U at scale C: semi-axes C·√λ along
/// the eigenvectors, longest first.
fn ellipsoid(u: Matrix3<f64>, scale: f64) -> Displacement {
    // Symmetric up to rounding; the eigensolver wants it exact.
    let symmetric = (u + u.transpose()) * 0.5;
    let eigen = SymmetricEigen::new(symmetric);
    let mut axes: Vec<(f64, [f64; 3])> = (0..3)
        .map(|i| {
            let v = eigen.eigenvectors.column(i);
            let length = scale * eigen.eigenvalues[i].max(0.0).sqrt();
            (length, [v[0] * length, v[1] * length, v[2] * length])
        })
        .collect();
    axes.sort_by(|a, b| b.0.total_cmp(&a.0));
    Displacement::Ellipsoid([axes[0].1, axes[1].1, axes[2].1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CellParams, Lattice, Measured, Occupant, Orientation, Rotation, Site, Species, SymOp,
        Symmetry,
    };

    fn site(id: u32, position: [f64; 3], adp: Option<Adp>) -> Site {
        let mut occupant = Occupant::new(
            &format!("A{id}"),
            Species::element("O"),
            Measured::exact(1.0),
        );
        occupant.adp = adp;
        Site::new(SiteId(id), position, vec![occupant])
    }

    fn aniso(u: [f64; 6]) -> Option<Adp> {
        Some(Adp::Anisotropic(u.map(Measured::exact)))
    }

    /// U_eq in Å²: a third of the trace of the Cartesian U, the isotropic
    /// parameter that `_atom_site_U_iso_or_equiv` gives for anisotropic atoms.
    fn equivalent(structure: &Structure, site: &Site) -> Option<f64> {
        match site.occupants.first()?.adp? {
            Adp::Isotropic(u) => Some(u.value()),
            Adp::Anisotropic(u) => {
                let basis = Matrix3::from(structure.lattice.matrix());
                let reciprocal = structure.lattice.params().reciprocal_lengths();
                let identity = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
                Some(cartesian(&u.map(|m| m.value()), &identity, &basis, &reciprocal).trace() / 3.0)
            }
        }
    }

    fn length(v: [f64; 3]) -> f64 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    /// ORTEP's table of C against the probability.
    #[test]
    fn probability_scales_match_ortep() {
        for (p, c) in [
            (0.01, 0.33887),
            (0.1, 0.76444),
            (0.3, 1.19317),
            (0.5, 1.53817),
            (0.7, 1.91439),
            (0.9, 2.50028),
            (0.99, 3.36821),
        ] {
            assert!(
                (probability_scale(p) - c).abs() < 1e-5,
                "{p}: {}",
                probability_scale(p)
            );
        }
        assert!(probability_scale(0.0) < 0.01);
        assert!(probability_scale(1.0) > 6.0);
    }

    /// U_iso gives a sphere of radius C·√U; missing and non-positive
    /// parameters are reported by site.
    #[test]
    fn isotropic_missing_and_not_positive() {
        let lattice = Lattice::new(CellParams::new(4.0, 4.0, 4.0, 90.0, 90.0, 90.0));
        let structure = Structure::new(
            lattice,
            Symmetry::p1(),
            vec![
                site(1, [0.0; 3], Some(Adp::Isotropic(Measured::exact(0.01)))),
                site(2, [0.5; 3], None),
                site(3, [0.25; 3], Some(Adp::Isotropic(Measured::exact(-0.001)))),
                site(4, [0.75; 3], aniso([0.01, 0.01, -0.02, 0.0, 0.0, 0.0])),
            ],
        );
        let displacements = Displacements::new(&structure, 0.5);
        let atom = |id: u32| AtomRef::new(SiteId(id), 0, [1, 0, 0]);
        let Displacement::Sphere(r) = displacements.get(atom(1)) else {
            panic!("{:?}", displacements.get(atom(1)));
        };
        assert!((r - 1.53817 * 0.1).abs() < 1e-5);
        assert_eq!(displacements.get(atom(2)), Displacement::Missing);
        assert_eq!(
            displacements.get(atom(3)),
            Displacement::NotPositiveDefinite
        );
        assert_eq!(
            displacements.get(atom(4)),
            Displacement::NotPositiveDefinite
        );
        assert_eq!(displacements.get(atom(9)), Displacement::Missing);
        assert_eq!(displacements.missing(), [SiteId(2)]);
        assert_eq!(
            displacements.not_positive_definite(),
            [SiteId(3), SiteId(4)]
        );
    }

    /// In an orthogonal cell U_ij are Cartesian: the axes are √U_ii along
    /// x, y, z, longest first.
    #[test]
    fn orthogonal_axes() {
        let lattice = Lattice::new(CellParams::new(3.0, 4.0, 5.0, 90.0, 90.0, 90.0));
        let structure = Structure::new(
            lattice,
            Symmetry::p1(),
            vec![site(
                1,
                [0.1, 0.2, 0.3],
                aniso([0.04, 0.01, 0.09, 0.0, 0.0, 0.0]),
            )],
        );
        let displacements = Displacements::new(&structure, 0.5);
        let Displacement::Ellipsoid(axes) = displacements.get(AtomRef::new(SiteId(1), 0, [0; 3]))
        else {
            panic!();
        };
        let c = probability_scale(0.5);
        for (axis, (expected, direction)) in axes.iter().zip([(0.3, 2), (0.2, 0), (0.1, 1)]) {
            assert!((length(*axis) - c * expected).abs() < 1e-9, "{axes:?}");
            assert!(
                (axis[direction].abs() - c * expected).abs() < 1e-9,
                "{axes:?}"
            );
        }
    }

    /// A mirror x ↔ y, as in the orbit of a 4-fold axis: the second member's
    /// ellipsoid is the first's with x and y swapped. U_eq is the same in
    /// any orientation of the lattice.
    #[test]
    fn members_turn_with_their_operation() {
        let params = CellParams::new(4.0, 4.0, 6.0, 90.0, 90.0, 90.0);
        let symmetry = Symmetry::new(vec![
            SymOp::parse("x,y,z").unwrap(),
            SymOp::parse("y,x,z").unwrap(),
        ])
        .unwrap();
        let sites = vec![site(
            1,
            [0.1, 0.3, 0.0],
            aniso([0.04, 0.01, 0.02, 0.0, 0.0, 0.0]),
        )];
        let structure = Structure::new(Lattice::new(params), symmetry.clone(), sites.clone());
        let displacements = Displacements::new(&structure, 0.5);
        let longest = |member: u16| match displacements.get(AtomRef::new(SiteId(1), member, [0; 3]))
        {
            Displacement::Ellipsoid(axes) => axes[0],
            other => panic!("{other:?}"),
        };
        assert!(longest(0)[0].abs() > 0.3 && longest(0)[1].abs() < 1e-9);
        assert!(longest(1)[1].abs() > 0.3 && longest(1)[0].abs() < 1e-9);

        // 30° about z: the ellipsoid turns, its size and U_eq do not.
        let (s, c) = (0.5, 3f64.sqrt() / 2.0);
        let rotation = Rotation::new([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]).unwrap();
        let turned = Structure::new(
            Lattice::with_orientation(params, Orientation::new(rotation)),
            symmetry,
            sites,
        );
        let turned_displacements = Displacements::new(&turned, 0.5);
        let Displacement::Ellipsoid(axes) =
            turned_displacements.get(AtomRef::new(SiteId(1), 0, [0; 3]))
        else {
            panic!();
        };
        let rows = rotation.rows();
        let expected = [0, 1, 2].map(|i| (0..3).map(|j| rows[i][j] * longest(0)[j]).sum::<f64>());
        assert!(
            (0..3).all(|i| (axes[0][i] - expected[i]).abs() < 1e-9)
                || (0..3).all(|i| (axes[0][i] + expected[i]).abs() < 1e-9),
            "{axes:?} against {expected:?}"
        );
        let ueq = (0.04 + 0.01 + 0.02) / 3.0;
        assert!((equivalent(&structure, &structure.sites[0]).unwrap() - ueq).abs() < 1e-12);
        assert!((equivalent(&turned, &turned.sites[0]).unwrap() - ueq).abs() < 1e-12);
    }

    /// Monoclinic: U_eq from the Cartesian U agrees with the CIF formula
    /// `U_eq = ⅓ Σᵢⱼ Uᵢⱼ aᵢ* aⱼ* aᵢ·aⱼ`, and the ellipsoid's volume with
    /// det U.
    #[test]
    fn skew_cells_match_the_cif_definitions() {
        let params = CellParams::new(5.0, 6.0, 7.0, 90.0, 110.0, 90.0);
        let u = [0.012, 0.018, 0.025, 0.0, 0.004, 0.0];
        let structure = Structure::new(
            Lattice::new(params),
            Symmetry::p1(),
            vec![site(1, [0.2, 0.3, 0.4], aniso(u))],
        );
        let reciprocal = params.reciprocal_lengths();
        let g = params.metric();
        let full = [[u[0], u[3], u[4]], [u[3], u[1], u[5]], [u[4], u[5], u[2]]];
        let mut ueq = 0.0;
        for i in 0..3 {
            for j in 0..3 {
                ueq += full[i][j] * reciprocal[i] * reciprocal[j] * g[i][j];
            }
        }
        ueq /= 3.0;
        assert!((equivalent(&structure, &structure.sites[0]).unwrap() - ueq).abs() < 1e-12);

        let displacements = Displacements::new(&structure, 0.5);
        let Displacement::Ellipsoid(axes) = displacements.get(AtomRef::new(SiteId(1), 0, [0; 3]))
        else {
            panic!();
        };
        // Perpendicular semi-axes whose product is C³·√det U.
        for i in 0..3 {
            for j in i + 1..3 {
                let d: f64 = (0..3).map(|k| axes[i][k] * axes[j][k]).sum();
                assert!(d.abs() < 1e-12);
            }
        }
        let basis = Matrix3::from(structure.lattice.matrix());
        let identity = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
        let det = cartesian(&u, &identity, &basis, &reciprocal).determinant();
        let product: f64 = axes.iter().map(|a| length(*a)).product();
        let c = probability_scale(0.5);
        assert!((product - c.powi(3) * det.sqrt()).abs() < 1e-12);
    }
}
