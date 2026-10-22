use serde::{Deserialize, Serialize};

use crate::{CellParam, Measured};

/// Cell lengths in Å and angles in degrees, kept as entered or imported so
/// that 90° stays exactly 90°.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellParams {
    pub a: Measured,
    pub b: Measured,
    pub c: Measured,
    pub alpha: Measured,
    pub beta: Measured,
    pub gamma: Measured,
}

impl CellParams {
    pub fn new(a: f64, b: f64, c: f64, alpha: f64, beta: f64, gamma: f64) -> Self {
        Self {
            a: a.into(),
            b: b.into(),
            c: c.into(),
            alpha: alpha.into(),
            beta: beta.into(),
            gamma: gamma.into(),
        }
    }

    pub fn get(&self, param: CellParam) -> Measured {
        match param {
            CellParam::A => self.a,
            CellParam::B => self.b,
            CellParam::C => self.c,
            CellParam::Alpha => self.alpha,
            CellParam::Beta => self.beta,
            CellParam::Gamma => self.gamma,
        }
    }

    pub fn set(&mut self, param: CellParam, value: Measured) {
        let field = match param {
            CellParam::A => &mut self.a,
            CellParam::B => &mut self.b,
            CellParam::C => &mut self.c,
            CellParam::Alpha => &mut self.alpha,
            CellParam::Beta => &mut self.beta,
            CellParam::Gamma => &mut self.gamma,
        };
        *field = value;
    }

    fn cosines(&self) -> (f64, f64, f64) {
        (
            cos_degrees(self.alpha.value()),
            cos_degrees(self.beta.value()),
            cos_degrees(self.gamma.value()),
        )
    }

    /// Positive lengths, angles strictly between 0° and 180°, and a positive
    /// definite metric tensor.
    pub fn is_valid(&self) -> bool {
        let lengths = [self.a, self.b, self.c].iter().all(|l| l.value() > 0.0);
        let angles = [self.alpha, self.beta, self.gamma]
            .iter()
            .all(|a| a.value() > 0.0 && a.value() < 180.0);
        lengths && angles && self.volume_factor() > 0.0
    }

    fn volume_factor(&self) -> f64 {
        let (ca, cb, cg) = self.cosines();
        1.0 - ca * ca - cb * cb - cg * cg + 2.0 * ca * cb * cg
    }

    /// Cell volume in Å³.
    pub fn volume(&self) -> f64 {
        self.a.value() * self.b.value() * self.c.value() * self.volume_factor().max(0.0).sqrt()
    }

    /// The metric tensor `G = Aᵀ·A` in Å², from the parameters directly so
    /// that right angles give exact zeros.
    pub(crate) fn metric(&self) -> [[f64; 3]; 3] {
        let lengths = [self.a.value(), self.b.value(), self.c.value()];
        let (ca, cb, cg) = self.cosines();
        let cosines = [[1.0, cg, cb], [cg, 1.0, ca], [cb, ca, 1.0]];
        [0, 1, 2].map(|i| [0, 1, 2].map(|j| lengths[i] * lengths[j] * cosines[i][j]))
    }

    /// Basis vectors a, b, c (as columns) in the standard orientation: a along
    /// x, b in the xy plane, right-handed.
    pub fn matrix(&self) -> [[f64; 3]; 3] {
        let (a, b, c) = (self.a.value(), self.b.value(), self.c.value());
        let (ca, cb, cg) = self.cosines();
        let sg = sin_degrees(self.gamma.value());
        let cy = (ca - cb * cg) / sg;
        let cz = (1.0 - cb * cb - cy * cy).max(0.0).sqrt();
        [
            [a, 0.0, 0.0],
            [b * cg, b * sg, 0.0],
            [c * cb, c * cy, c * cz],
        ]
    }
}

/// The length in Å of a fractional vector under the metric tensor `g`.
pub(crate) fn norm(g: &[[f64; 3]; 3], v: [f64; 3]) -> f64 {
    let mut sum = 0.0;
    for i in 0..3 {
        for j in 0..3 {
            sum += v[i] * g[i][j] * v[j];
        }
    }
    sum.max(0.0).sqrt()
}

/// Cosine that is exact for the angles crystallography uses most, so that a
/// right angle never turns into 6e-17.
fn cos_degrees(degrees: f64) -> f64 {
    match degrees {
        60.0 => 0.5,
        90.0 => 0.0,
        120.0 => -0.5,
        _ => degrees.to_radians().cos(),
    }
}

fn sin_degrees(degrees: f64) -> f64 {
    match degrees {
        90.0 => 1.0,
        _ => degrees.to_radians().sin(),
    }
}

/// The crystal lattice. Only the standard orientation exists so far; an
/// explicit orientation for POSCAR imports comes with file formats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lattice {
    params: CellParams,
}

impl Lattice {
    pub fn new(params: CellParams) -> Self {
        Self { params }
    }

    pub fn params(&self) -> &CellParams {
        &self.params
    }

    /// Replaces the cell parameters, keeping the orientation.
    pub fn set_params(&mut self, params: CellParams) {
        self.params = params;
    }

    /// Converts fractional coordinates to Cartesian Å.
    pub fn to_cartesian(&self, fractional: [f64; 3]) -> [f64; 3] {
        let m = self.params.matrix();
        let mut r = [0.0; 3];
        for (axis, basis) in m.iter().enumerate() {
            for (i, component) in basis.iter().enumerate() {
                r[i] += fractional[axis] * component;
            }
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn tetragonal_volume() {
        let cell = CellParams::new(4.594, 4.594, 2.959, 90.0, 90.0, 90.0);
        assert!(close(cell.volume(), 4.594 * 4.594 * 2.959));
    }

    #[test]
    fn right_angles_give_an_exactly_diagonal_matrix() {
        let m = CellParams::new(3.0, 4.0, 5.0, 90.0, 90.0, 90.0).matrix();
        assert_eq!(m, [[3.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 5.0]]);
    }

    #[test]
    fn hexagonal_b_axis_is_exact() {
        let m = CellParams::new(3.0, 3.0, 5.0, 90.0, 90.0, 120.0).matrix();
        assert_eq!(m[1][0], -1.5);
        assert!(close(m[1][1], 3.0 * (3.0f64).sqrt() / 2.0));
    }

    #[test]
    fn rejects_impossible_angles() {
        assert!(!CellParams::new(1.0, 1.0, 1.0, 10.0, 10.0, 170.0).is_valid());
        assert!(CellParams::new(1.0, 1.0, 1.0, 90.0, 90.0, 90.0).is_valid());
    }

    #[test]
    fn body_center_maps_to_half_diagonal() {
        let lattice = Lattice::new(CellParams::new(2.0, 4.0, 6.0, 90.0, 90.0, 90.0));
        assert_eq!(lattice.to_cartesian([0.5, 0.5, 0.5]), [1.0, 2.0, 3.0]);
    }
}
