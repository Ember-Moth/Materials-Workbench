//! Site symmetry (design §5.2): the operations that leave a position in
//! place, the constraint they put on its coordinates, and the special
//! position a point near one snaps to (design §7.6).

use std::collections::HashMap;
use std::fmt;

use crate::lattice::norm;
use crate::{Axis, CoordinateConstraint, Rational, Structure, Symmetry, Wyckoff, linalg};

/// The site-symmetry group G_s of a position: the operations g with
/// `g(x) ≡ x` modulo lattice translations, within a tolerance. Each member
/// keeps the lattice translation that brings its image back next to x, so
/// that `W·x + t ≈ x` with no rounding.
pub(crate) struct Stabilizer {
    members: Vec<([[i32; 3]; 3], [Rational; 3])>,
}

impl Stabilizer {
    /// The operations that move `x` by less than `tolerance` Å. Should they
    /// not form a group, which needs a tolerance close to the distance
    /// between special positions, the tolerance shrinks until they do.
    pub fn new(symmetry: &Symmetry, metric: &[[f64; 3]; 3], x: [f64; 3], tolerance: f64) -> Self {
        let mut tolerance = tolerance;
        for _ in 0..8 {
            let stabilizer = Self::within(symmetry, metric, x, tolerance);
            if stabilizer.is_group() {
                return stabilizer;
            }
            tolerance /= 10.0;
        }
        Self {
            members: vec![([[1, 0, 0], [0, 1, 0], [0, 0, 1]], [Rational::ZERO; 3])],
        }
    }

    fn within(symmetry: &Symmetry, metric: &[[f64; 3]; 3], x: [f64; 3], tolerance: f64) -> Self {
        let members = symmetry
            .operations()
            .iter()
            .filter_map(|op| {
                let image = op.apply(x);
                let shift = [0, 1, 2].map(|i| (x[i] - image[i]).round());
                let offset = [0, 1, 2].map(|i| image[i] + shift[i] - x[i]);
                (norm(metric, offset) < tolerance).then(|| {
                    let translation =
                        [0, 1, 2].map(|i| op.translation()[i] + Rational::integer(shift[i] as i64));
                    (*op.rotation(), translation)
                })
            })
            .collect();
        Self { members }
    }

    /// Closed under composition, with the lattice translations included:
    /// an exact fixed point has exactly one translation per rotation.
    fn is_group(&self) -> bool {
        let by_rotation: HashMap<[[i32; 3]; 3], [Rational; 3]> =
            self.members.iter().copied().collect();
        by_rotation.len() == self.members.len()
            && self.members.iter().all(|(w1, t1)| {
                self.members.iter().all(|(w2, t2)| {
                    let w = [0, 1, 2]
                        .map(|i| [0, 1, 2].map(|j| (0..3).map(|k| w1[i][k] * w2[k][j]).sum()));
                    let t = [0, 1, 2]
                        .map(|i| (0..3).fold(t1[i], |sum, k| sum + t2[k] * i64::from(w1[i][k])));
                    by_rotation.get(&w) == Some(&t)
                })
            })
    }

    pub fn order(&self) -> usize {
        self.members.len()
    }

    /// `P_s`, the average of the linear parts, and the average translation
    /// `t̄`. The special position is `{x : x = P_s·x + t̄}`, i.e. `t̄` plus
    /// the range of `P_s`.
    fn averages(&self) -> ([[Rational; 3]; 3], [Rational; 3]) {
        let n = Rational::integer(self.members.len() as i64);
        let mut projector = [[Rational::ZERO; 3]; 3];
        let mut translation = [Rational::ZERO; 3];
        for (w, t) in &self.members {
            for i in 0..3 {
                for j in 0..3 {
                    projector[i][j] = projector[i][j] + Rational::integer(i64::from(w[i][j]));
                }
                translation[i] = translation[i] + t[i];
            }
        }
        (
            projector.map(|row| row.map(|v| v / n)),
            translation.map(|v| v / n),
        )
    }

    /// `x_sym = P_s·x + t̄`: the average of x's images (design §7.6).
    pub(crate) fn snap(&self, x: [f64; 3]) -> [f64; 3] {
        let (projector, translation) = self.averages();
        [0, 1, 2].map(|i| {
            (0..3).map(|j| projector[i][j].to_f64() * x[j]).sum::<f64>() + translation[i].to_f64()
        })
    }

    pub(crate) fn form(&self) -> CoordinateForm {
        let (projector, translation) = self.averages();
        CoordinateForm::new(&projector, translation)
    }

    pub(crate) fn projector(&self) -> [[f64; 3]; 3] {
        self.averages().0.map(|row| row.map(Rational::to_f64))
    }
}

/// One coordinate of a special position: a combination of the free
/// coordinates plus a constant in [0, 1).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Coordinate {
    coefficients: [Rational; 3],
    constant: Rational,
}

impl fmt::Display for Coordinate {
    /// ITA notation: `x`, `-x+1/2`, `2x`, `x/2`, `1/4`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut text = String::new();
        for (c, axis) in self.coefficients.iter().zip(["x", "y", "z"]) {
            if c.is_zero() {
                continue;
            }
            text.push(if c.numerator() < 0 { '-' } else { '+' });
            let (num, den) = (c.numerator().abs(), c.denominator());
            if num != 1 {
                text.push_str(&num.to_string());
            }
            text.push_str(axis);
            if den != 1 {
                text.push_str(&format!("/{den}"));
            }
        }
        if !self.constant.is_zero() {
            text.push_str(&format!("+{}", self.constant));
        }
        let text = text.strip_prefix('+').unwrap_or(&text);
        f.write_str(if text.is_empty() { "0" } else { text })
    }
}

/// The coordinates a site can take without leaving its special position,
/// as the International Tables write them: `x, x, 0` or `x, 2x, 1/4`
/// (design §5.2). Free coordinates are the pivots of the row-reduced basis
/// of the range of `P_s`; the others follow from them. Works in any
/// setting, without the Wyckoff tables.
#[derive(Clone, Debug, PartialEq)]
pub struct CoordinateForm([Coordinate; 3]);

impl CoordinateForm {
    fn new(projector: &[[Rational; 3]; 3], translation: [Rational; 3]) -> Self {
        // The columns of P_s span its range.
        let mut basis: Vec<[Rational; 3]> =
            (0..3).map(|j| [0, 1, 2].map(|i| projector[i][j])).collect();
        let pivots = linalg::rref(&mut basis);
        Self([0, 1, 2].map(|j| {
            if let Some(k) = pivots.iter().position(|&p| p == j) {
                let mut coefficients = [Rational::ZERO; 3];
                coefficients[pivots[k]] = Rational::ONE;
                return Coordinate {
                    coefficients,
                    constant: Rational::ZERO,
                };
            }
            // x_j = t̄_j + Σ_k b_k[j]·(x_{p_k} − t̄_{p_k}).
            let mut coefficients = [Rational::ZERO; 3];
            let mut constant = translation[j];
            for (row, &p) in basis.iter().zip(&pivots) {
                coefficients[p] = row[j];
                constant = constant - row[j] * translation[p];
            }
            Coordinate {
                coefficients,
                constant: constant.fract(),
            }
        }))
    }

    /// How many coordinates can be chosen freely: 3 on a general position,
    /// 0 on a point such as an inversion center.
    pub fn free_parameters(&self) -> usize {
        self.constraints()
            .iter()
            .filter(|c| **c == CoordinateConstraint::Free)
            .count()
    }

    /// The expression for one coordinate, e.g. `2x` for y in `x, 2x, 1/4`.
    pub fn expression(&self, axis: Axis) -> String {
        self.0[axis as usize].to_string()
    }

    /// The point of this special position whose `axis` coordinate is
    /// `value`, near `current` (design §7.5). A free coordinate takes the
    /// value; one that follows a single free coordinate gives that one the
    /// value that reproduces it. The coordinates that depend on the changed
    /// free coordinate follow it, keeping their lattice translation from
    /// `current` (y stays 0.75 rather than −0.25 in `x, −x, 1/2`); the
    /// others, and the typed coordinate itself, keep their values exactly.
    /// A fixed coordinate, or a combination of free ones, cannot be set:
    /// the error is its constraint.
    pub(crate) fn with_coordinate(
        &self,
        current: [f64; 3],
        axis: Axis,
        value: f64,
    ) -> Result<[f64; 3], CoordinateConstraint> {
        let at = |i: usize, free: &[f64; 3]| {
            let coordinate = &self.0[i];
            (0..3)
                .map(|k| coordinate.coefficients[k].to_f64() * free[k])
                .sum::<f64>()
                + coordinate.constant.to_f64()
        };
        let j = axis as usize;
        let changed = match self.constraints()[j] {
            CoordinateConstraint::Free => j,
            CoordinateConstraint::Follows(free) => free as usize,
            other => return Err(other),
        };
        // Free coordinates are their own values, so `current` serves as
        // the free parameters.
        let offset = |i: usize| (current[i] - at(i, &current)).round();
        let mut free = current;
        free[changed] = if changed == j {
            value
        } else {
            let coordinate = &self.0[j];
            (value - offset(j) - coordinate.constant.to_f64())
                / coordinate.coefficients[changed].to_f64()
        };
        let mut position = [0, 1, 2].map(|i| {
            if i == changed {
                free[i]
            } else if self.0[i].coefficients[changed].is_zero() {
                current[i]
            } else {
                at(i, &free) + offset(i)
            }
        });
        position[j] = value;
        Ok(position)
    }

    pub fn constraints(&self) -> [CoordinateConstraint; 3] {
        [0, 1, 2].map(|j| {
            let coordinate = &self.0[j];
            let used: Vec<usize> = (0..3)
                .filter(|&k| !coordinate.coefficients[k].is_zero())
                .collect();
            match used[..] {
                [] => CoordinateConstraint::Fixed,
                [k] if k == j => CoordinateConstraint::Free,
                [k] => CoordinateConstraint::Follows(Axis::ALL[k]),
                _ => CoordinateConstraint::Combination,
            }
        })
    }
}

impl fmt::Display for CoordinateForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}, {}, {}", self.0[0], self.0[1], self.0[2])
    }
}

/// The symmetry of one site (design §5.2): its site-symmetry group, how
/// that constrains the coordinates, and its Wyckoff position.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SiteSymmetry {
    /// |G_s|, the number of operations that leave the site in place. The
    /// site has |operations| / |G_s| equivalent atoms in the cell.
    pub order: usize,
    /// `P_s`, the average of the linear parts of G_s, in fractional
    /// coordinates: projects a displacement onto the directions that keep
    /// the site on its special position (design §7.4).
    pub projector: [[f64; 3]; 3],
    pub form: CoordinateForm,
    pub coordinates: [CoordinateConstraint; 3],
    /// `None` when the space group type is not identified or no tabulated
    /// position matches.
    pub wyckoff: Option<Wyckoff>,
}

impl SiteSymmetry {
    pub(crate) fn new(stabilizer: &Stabilizer, wyckoff: Option<Wyckoff>) -> Self {
        let form = stabilizer.form();
        Self {
            order: stabilizer.order(),
            projector: stabilizer.projector(),
            coordinates: form.constraints(),
            form,
            wyckoff,
        }
    }
}

/// Where a position belongs when it lies near a special position
/// (design §7.6, §9.1).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SpecialPosition {
    /// The average of the position's images under its site-symmetry group,
    /// each taken at the lattice translation nearest to it, so the result
    /// stays next to the given position. The position itself when the
    /// group holds only the identity.
    pub position: [f64; 3],
    /// Å, from the given position to `position`.
    pub displacement: f64,
    /// |G_s|.
    pub order: usize,
    pub form: CoordinateForm,
}

/// The special position within the structure's position tolerance of
/// `position`, under the structure's operations.
pub fn special_position(structure: &Structure, position: [f64; 3]) -> SpecialPosition {
    let metric = structure.lattice.params().metric();
    let stabilizer = Stabilizer::new(
        &structure.symmetry,
        &metric,
        position,
        structure.tolerances.position,
    );
    let snapped = stabilizer.snap(position);
    SpecialPosition {
        displacement: norm(&metric, [0, 1, 2].map(|i| snapped[i] - position[i])),
        position: snapped,
        order: stabilizer.order(),
        form: stabilizer.form(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellParams, Lattice, Site, SymOp};

    fn structure(generators: &[&str], params: CellParams, sites: Vec<Site>) -> Structure {
        let generators: Vec<SymOp> = generators
            .iter()
            .map(|g| SymOp::parse(g).unwrap())
            .collect();
        Structure::new(
            Lattice::new(params),
            Symmetry::generate(&generators).unwrap(),
            sites,
        )
    }

    fn rutile(x: [f64; 3]) -> Structure {
        structure(
            &["-y+1/2,x+1/2,z+1/2", "-x+1/2,y+1/2,-z+1/2", "-x,-y,-z"],
            CellParams::new(4.594, 4.594, 2.959, 90.0, 90.0, 90.0),
            vec![Site::single(1, "O1", "O", x)],
        )
    }

    fn hexagonal(x: [f64; 3]) -> Structure {
        // P6₃/mmc (194).
        structure(
            &["x-y,x,z+1/2", "y,x,-z", "-x,-y,-z"],
            CellParams::new(3.0, 3.0, 5.0, 90.0, 90.0, 120.0),
            vec![Site::single(1, "A", "C", x)],
        )
    }

    fn analyze(structure: &Structure) -> (usize, String, [CoordinateConstraint; 3]) {
        let metric = structure.lattice.params().metric();
        let stabilizer = Stabilizer::new(
            &structure.symmetry,
            &metric,
            structure.sites[0].position,
            0.01,
        );
        let site = SiteSymmetry::new(&stabilizer, None);
        (site.order, site.form.to_string(), site.coordinates)
    }

    use CoordinateConstraint::{Fixed, Follows, Free};

    #[test]
    fn rutile_positions() {
        assert_eq!(
            analyze(&rutile([0.3048, 0.3048, 0.0])),
            (4, "x, x, 0".into(), [Free, Follows(Axis::X), Fixed])
        );
        assert_eq!(
            analyze(&rutile([0.0, 0.0, 0.0])),
            (8, "0, 0, 0".into(), [Fixed; 3])
        );
        assert_eq!(
            analyze(&rutile([0.5, 0.5, 0.5])),
            (8, "1/2, 1/2, 1/2".into(), [Fixed; 3])
        );
        assert_eq!(
            analyze(&rutile([0.1, 0.2, 0.3])),
            (1, "x, y, z".into(), [Free; 3])
        );
        // Another member of 4f, -x+1/2, x+1/2, 1/2 with x = 0.3: constants
        // are taken modulo 1.
        assert_eq!(analyze(&rutile([0.2, 0.8, 0.5])).1, "x, -x, 1/2");
        // 4g, x, -x, 0.
        assert_eq!(analyze(&rutile([0.3, 0.7, 0.0])).1, "x, -x, 0");
    }

    #[test]
    fn hexagonal_positions_need_row_reduction() {
        // 6h of P6₃/mmc: the diagonal of P_s is no guide here (design §7.5).
        let (order, form, constraints) = analyze(&hexagonal([0.17, 0.34, 0.25]));
        assert_eq!((order, form.as_str()), (4, "x, 2x, 1/4"));
        assert_eq!(constraints, [Free, Follows(Axis::X), Fixed]);
        assert_eq!(
            analyze(&hexagonal([1.0 / 3.0, 2.0 / 3.0, 0.25])).1,
            "1/3, 2/3, 1/4"
        );
        assert_eq!(analyze(&hexagonal([0.0, 0.0, 0.1])).1, "0, 0, z");
    }

    #[test]
    fn projector_rank_is_the_number_of_free_coordinates() {
        let s = rutile([0.3048, 0.3048, 0.0]);
        let metric = s.lattice.params().metric();
        let site = SiteSymmetry::new(
            &Stabilizer::new(&s.symmetry, &metric, s.sites[0].position, 0.01),
            None,
        );
        assert_eq!(site.form.free_parameters(), 1);
        // P_s projects onto the [110] direction.
        assert_eq!(
            site.projector,
            [[0.5, 0.5, 0.0], [0.5, 0.5, 0.0], [0.0, 0.0, 0.0]]
        );
    }

    #[test]
    fn snaps_within_the_tolerance_only() {
        // 0.0013 Å off the 4f line x, x, 0 in the plane, 0.003 Å along c.
        let s = rutile([0.3046, 0.3050, 0.001]);
        let snapped = special_position(&s, s.sites[0].position);
        assert_eq!(snapped.order, 4);
        assert_eq!(snapped.form.to_string(), "x, x, 0");
        for (a, b) in snapped.position.iter().zip([0.3048, 0.3048, 0.0]) {
            assert!((a - b).abs() < 1e-12, "{:?}", snapped.position);
        }
        assert!(
            (snapped.displacement - 0.00323).abs() < 1e-5,
            "{}",
            snapped.displacement
        );

        // 0.03 Å off the mirror z = 0: left alone.
        let s = rutile([0.3048, 0.3048, 0.01]);
        let kept = special_position(&s, s.sites[0].position);
        assert_eq!(kept.order, 2);
        assert_eq!(kept.position[2], 0.01);
    }

    #[test]
    fn snapping_keeps_the_lattice_translation() {
        // Next to the origin image at (1, 0, -1): stays there (design §3).
        let s = rutile([0.9995, 0.0004, -1.0002]);
        let snapped = special_position(&s, s.sites[0].position);
        assert_eq!(snapped.order, 8);
        assert_eq!(snapped.position, [1.0, 0.0, -1.0]);
    }
}
