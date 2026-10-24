//! How symmetry constrains the cell (design §7.3): every operation must
//! preserve the metric tensor, `Wᵀ·G·W = G`. The metrics that satisfy this
//! form a linear space; which parameters are free, linked or fixed follows
//! from the linear relations that hold on it. Works in any setting.

use crate::{CellConstraints, CellParam, CellParams, Constraint, Rational, Symmetry, linalg};

/// Components of the symmetric metric as a vector: g11, g22, g33, g12,
/// g13, g23.
const COMPONENTS: [(usize, usize); 6] = [(0, 0), (1, 1), (2, 2), (0, 1), (0, 2), (1, 2)];

fn component(i: usize, j: usize) -> usize {
    let (i, j) = (i.min(j), i.max(j));
    COMPONENTS.iter().position(|&c| c == (i, j)).expect("3 × 3")
}

/// The distinct linear parts of the operations.
fn rotations(symmetry: &Symmetry) -> Vec<[[i32; 3]; 3]> {
    let mut rotations: Vec<[[i32; 3]; 3]> = Vec::new();
    for op in symmetry.operations() {
        if !rotations.contains(op.rotation()) {
            rotations.push(*op.rotation());
        }
    }
    rotations
}

/// A basis of the metrics the operations preserve.
fn invariant_metrics(symmetry: &Symmetry) -> Vec<[Rational; 6]> {
    let mut equations = Vec::new();
    for w in rotations(symmetry) {
        // (Wᵀ·G·W)_ij − G_ij = Σ_ab W_ai·G_ab·W_bj − G_ij = 0 for i ≤ j.
        for &(i, j) in &COMPONENTS {
            let mut row = [Rational::ZERO; 6];
            for a in 0..3 {
                for b in 0..3 {
                    let k = component(a, b);
                    row[k] = row[k] + Rational::integer(i64::from(w[a][i] * w[b][j]));
                }
            }
            let k = component(i, j);
            row[k] = row[k] - Rational::ONE;
            equations.push(row);
        }
    }
    linalg::null_space(&equations)
}

/// Which cell parameters the operations leave free, link to another or
/// fix. Lengths link when their squares are equal on every invariant
/// metric; an angle is fixed when its cosine is 0 (90°) or ∓½ between
/// equal lengths (120°, 60°), and linked to another when both cosines
/// always agree, as for rhombohedral axes.
pub(crate) fn cell_constraints(symmetry: &Symmetry) -> CellConstraints {
    let space = invariant_metrics(symmetry);
    let holds = |terms: &[(usize, Rational)]| {
        let mut relation = [Rational::ZERO; 6];
        for &(k, c) in terms {
            relation[k] = relation[k] + c;
        }
        linalg::annihilates(&relation, &space)
    };
    let one = Rational::ONE;
    let half = Rational::new(1, 2);
    // g_ii = g_jj.
    let equal_lengths =
        |i: usize, j: usize| holds(&[(component(i, i), one), (component(j, j), -one)]);
    // g_ij = c·g_ii for a constant c.
    let off_diagonal =
        |i: usize, j: usize, c: Rational| holds(&[(component(i, j), one), (component(i, i), -c)]);
    // The angle between axes i and j.
    let fixed_angle = |i: usize, j: usize| {
        off_diagonal(i, j, Rational::ZERO)
            || (equal_lengths(i, j) && (off_diagonal(i, j, -half) || off_diagonal(i, j, half)))
    };
    // The angles (i, j) and (k, l) between equal pairs of lengths.
    let equal_angles = |(i, j): (usize, usize), (k, l): (usize, usize), lengths: (usize, usize)| {
        holds(&[(component(i, j), one), (component(k, l), -one)])
            && equal_lengths(lengths.0, lengths.1)
    };

    let b = if equal_lengths(1, 0) {
        Constraint::Equals(CellParam::A)
    } else {
        Constraint::Free
    };
    let c = if equal_lengths(2, 0) {
        Constraint::Equals(CellParam::A)
    } else if equal_lengths(2, 1) {
        Constraint::Equals(CellParam::B)
    } else {
        Constraint::Free
    };
    // α is between b and c, β between a and c, γ between a and b.
    let alpha = if fixed_angle(1, 2) {
        Constraint::Fixed
    } else {
        Constraint::Free
    };
    let beta = if fixed_angle(0, 2) {
        Constraint::Fixed
    } else if equal_angles((0, 2), (1, 2), (0, 1)) {
        Constraint::Equals(CellParam::Alpha)
    } else {
        Constraint::Free
    };
    let gamma = if fixed_angle(0, 1) {
        Constraint::Fixed
    } else if equal_angles((0, 1), (1, 2), (0, 2)) {
        Constraint::Equals(CellParam::Alpha)
    } else if equal_angles((0, 1), (0, 2), (1, 2)) {
        Constraint::Equals(CellParam::Beta)
    } else {
        Constraint::Free
    };
    CellConstraints::new([Constraint::Free, b, c, alpha, beta, gamma])
}

/// The average of `Wᵀ·G·W` over the point group: the nearest metric the
/// operations preserve.
pub(crate) fn symmetrized_metric(symmetry: &Symmetry, metric: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let rotations = rotations(symmetry);
    let mut sum = [[0.0; 3]; 3];
    for w in &rotations {
        for i in 0..3 {
            for j in 0..3 {
                for a in 0..3 {
                    for b in 0..3 {
                        sum[i][j] += f64::from(w[a][i]) * metric[a][b] * f64::from(w[b][j]);
                    }
                }
            }
        }
    }
    sum.map(|row| row.map(|v| v / rotations.len() as f64))
}

/// Differences between the cell and the nearest cell the operations
/// preserve below this (Å or degrees) are rounding, not inconsistency: in
/// the COD, a third of the cells that differ at all differ by less, such as
/// b = 32.246 written for a = 32.2456(13). Above it the importer warns and
/// the interface offers to align the cell (design §7.3).
pub const CELL_DEVIATION: f64 = 1e-3;

/// How far the cell parameters are from the nearest cell the operations
/// preserve: the largest difference of a length in Å and of an angle in
/// degrees. Both are zero, up to rounding, for a consistent cell.
pub fn cell_deviation(symmetry: &Symmetry, params: &CellParams) -> (f64, f64) {
    let actual = params.metric();
    let ideal = symmetrized_metric(symmetry, actual);
    let lengths = |g: &[[f64; 3]; 3]| [0, 1, 2].map(|i| g[i][i].sqrt());
    let angles = |g: &[[f64; 3]; 3]| {
        let l = lengths(g);
        [(1, 2), (0, 2), (0, 1)].map(|(i, j)| {
            (g[i][j] / (l[i] * l[j]))
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees()
        })
    };
    let max_difference = |a: [f64; 3], b: [f64; 3]| {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f64::max)
    };
    (
        max_difference(lengths(&actual), lengths(&ideal)),
        max_difference(angles(&actual), angles(&ideal)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SymOp;
    use Constraint::{Equals, Fixed, Free};

    fn constraints(generators: &[&str]) -> [Constraint; 6] {
        let generators: Vec<SymOp> = generators
            .iter()
            .map(|g| SymOp::parse(g).unwrap())
            .collect();
        let c = cell_constraints(&Symmetry::generate(&generators).unwrap());
        CellParam::ALL.map(|p| c.get(p))
    }

    #[test]
    fn constraints_by_crystal_system() {
        assert_eq!(constraints(&["-x,-y,-z"]), [Free; 6]);
        assert_eq!(
            constraints(&["-x,y+1/2,-z+1/2", "-x,-y,-z"]),
            [Free, Free, Free, Fixed, Free, Fixed]
        );
        assert_eq!(
            constraints(&["-x,-y,z", "x,-y,-z"]),
            [Free, Free, Free, Fixed, Fixed, Fixed]
        );
        let a = Equals(CellParam::A);
        assert_eq!(
            constraints(&["-y,x,z+1/2", "-x,-y,-z"]),
            [Free, a, Free, Fixed, Fixed, Fixed]
        );
        assert_eq!(
            constraints(&["x-y,x,z+1/2", "y,x,-z"]),
            [Free, a, Free, Fixed, Fixed, Fixed]
        );
        assert_eq!(
            constraints(&["-y,x,z", "z,x,y", "-x,-y,-z"]),
            [Free, a, a, Fixed, Fixed, Fixed]
        );
        // Rhombohedral axes: a = b = c and α = β = γ, α free.
        let alpha = Equals(CellParam::Alpha);
        assert_eq!(
            constraints(&["z,x,y", "-y,-x,-z", "-x,-y,-z"]),
            [Free, a, a, Free, alpha, alpha]
        );
    }

    #[test]
    fn deviation_of_an_inconsistent_cell() {
        let cubic = Symmetry::from_it_number(221).unwrap();
        let exact = CellParams::new(3.905, 3.905, 3.905, 90.0, 90.0, 90.0);
        let (length, angle) = cell_deviation(&cubic, &exact);
        assert!(length < 1e-12 && angle < 1e-12);
        let off = CellParams::new(3.905, 3.9053, 3.905, 90.0, 90.0, 90.0);
        let (length, _) = cell_deviation(&cubic, &off);
        assert!((length - 0.0002).abs() < 1e-6, "{length}");
        let hexagonal = Symmetry::from_it_number(194).unwrap();
        let skewed = CellParams::new(3.0, 3.0, 5.0, 90.0, 90.0, 119.9);
        let (_, angle) = cell_deviation(&hexagonal, &skewed);
        assert!(angle > 0.05, "{angle}");
    }
}
