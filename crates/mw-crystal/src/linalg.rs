//! Exact linear algebra over the rationals, for the small systems that
//! symmetry analysis solves: row reduction and null spaces of matrices with
//! at most a few dozen rows of 3 or 6 columns.

use crate::Rational;

/// Brings `rows` into reduced row echelon form, drops the zero rows and
/// returns each remaining row's pivot column. Every pivot is 1 and is the
/// only nonzero entry in its column.
pub(crate) fn rref<const N: usize>(rows: &mut Vec<[Rational; N]>) -> Vec<usize> {
    let mut pivots = Vec::new();
    for column in 0..N {
        let r = pivots.len();
        let Some(found) = (r..rows.len()).find(|&i| !rows[i][column].is_zero()) else {
            continue;
        };
        rows.swap(r, found);
        let pivot = rows[r][column];
        let pivot_row = rows[r].map(|v| v / pivot);
        rows[r] = pivot_row;
        for (i, row) in rows.iter_mut().enumerate() {
            let factor = row[column];
            if i != r && !factor.is_zero() {
                for (v, p) in row.iter_mut().zip(pivot_row) {
                    *v = *v - factor * p;
                }
            }
        }
        pivots.push(column);
    }
    rows.truncate(pivots.len());
    pivots
}

/// A basis of `{ v : M·v = 0 }`, where `rows` are the rows of `M`.
pub(crate) fn null_space<const N: usize>(rows: &[[Rational; N]]) -> Vec<[Rational; N]> {
    let mut reduced = rows.to_vec();
    let pivots = rref(&mut reduced);
    (0..N)
        .filter(|column| !pivots.contains(column))
        .map(|free| {
            let mut v = [Rational::ZERO; N];
            v[free] = Rational::ONE;
            for (row, &pivot) in reduced.iter().zip(&pivots) {
                v[pivot] = -row[free];
            }
            v
        })
        .collect()
}

/// Whether `v` is orthogonal to every vector of `basis`, i.e. the linear
/// relation `v·x = 0` holds on the space they span.
pub(crate) fn annihilates<const N: usize>(v: &[Rational; N], basis: &[[Rational; N]]) -> bool {
    basis.iter().all(|b| {
        v.iter()
            .zip(b)
            .fold(Rational::ZERO, |sum, (x, y)| sum + *x * *y)
            .is_zero()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: i64) -> Rational {
        Rational::integer(n)
    }

    #[test]
    fn reduces_to_echelon_form() {
        let mut rows = vec![[r(2), r(4), r(0)], [r(1), r(2), r(0)], [r(0), r(0), r(3)]];
        let pivots = rref(&mut rows);
        assert_eq!(pivots, [0, 2]);
        assert_eq!(rows, [[r(1), r(2), r(0)], [r(0), r(0), r(1)]]);
    }

    #[test]
    fn null_space_of_a_plane() {
        // x + y = 0 leaves (−1, 1, 0) and (0, 0, 1).
        let basis = null_space(&[[r(1), r(1), r(0)]]);
        assert_eq!(basis, [[r(-1), r(1), r(0)], [r(0), r(0), r(1)]]);
        assert!(annihilates(&[r(1), r(1), r(0)], &basis));
        assert!(!annihilates(&[r(1), r(0), r(0)], &basis));
    }
}
