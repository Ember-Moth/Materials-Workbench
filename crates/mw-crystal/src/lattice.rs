use nalgebra::Matrix3;
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

    /// a*, b*, c*: the lengths of the reciprocal basis, in Å⁻¹.
    pub fn reciprocal_lengths(&self) -> [f64; 3] {
        let g = Matrix3::from(self.metric()).transpose();
        let inverse = g.try_inverse().unwrap_or_else(Matrix3::identity);
        [0, 1, 2].map(|i| inverse[(i, i)].sqrt())
    }

    /// Basis vectors a, b, c in Å in the standard orientation (design §4.2):
    /// a along x, b in the xy plane, right-handed. `m[i]` is the i-th basis
    /// vector, i.e. the i-th column of `A_std`. Cartesian positions come
    /// from [`Lattice::matrix`], which applies the lattice's orientation.
    pub fn standard_matrix(&self) -> [[f64; 3]; 3] {
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

    /// The parameters of the metric tensor `g` in Å². Cosines within
    /// rounding of 0 or ±½ and angles within rounding of a whole degree are
    /// taken exactly, so that a cell given by vectors keeps its right angles.
    pub(crate) fn from_metric(g: [[f64; 3]; 3]) -> Self {
        let lengths = [0, 1, 2].map(|i| g[i][i].sqrt());
        let angle = |i: usize, j: usize| {
            let mut cosine = g[i][j] / (lengths[i] * lengths[j]);
            if let Some(exact) = [0.0, 0.5, -0.5]
                .into_iter()
                .find(|exact| (cosine - exact).abs() < ROUNDING)
            {
                cosine = exact;
            }
            let degrees = cosine.acos().to_degrees();
            let rounded = degrees.round();
            if (degrees - rounded).abs() < ROUNDING {
                rounded
            } else {
                degrees
            }
        };
        Self::new(
            lengths[0],
            lengths[1],
            lengths[2],
            angle(1, 2),
            angle(0, 2),
            angle(0, 1),
        )
    }
}

/// Cosines and angles this close to an exact value are that value.
const ROUNDING: f64 = 1e-9;

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

/// Below this, a difference is rounding rather than a real rotation: a
/// matrix this close to orthogonal is a [`Rotation`], and a rotation this
/// close to the identity is the standard orientation.
const ORIENTATION_TOLERANCE: f64 = 1e-9;

/// An orthogonal matrix that places a cell in Cartesian space: a proper
/// rotation, or one combined with an inversion (det = −1) for a left-handed
/// cell, as the rotation part of a symmetry operation may be improper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rotation([[f64; 3]; 3]);

impl Rotation {
    /// The matrix with these rows, if it is orthogonal within rounding. The
    /// entries are kept as given.
    pub fn new(rows: [[f64; 3]; 3]) -> Option<Self> {
        let finite = rows.iter().flatten().all(|v| v.is_finite());
        let orthogonal = (0..3).all(|i| {
            (0..3).all(|j| {
                let dot: f64 = (0..3).map(|k| rows[i][k] * rows[j][k]).sum();
                let expected = if i == j { 1.0 } else { 0.0 };
                (dot - expected).abs() <= ORIENTATION_TOLERANCE
            })
        });
        (finite && orthogonal).then_some(Self(rows))
    }

    pub fn rows(&self) -> &[[f64; 3]; 3] {
        &self.0
    }

    /// +1 for a proper rotation, −1 for one that turns right-handed axes
    /// into left-handed ones (within rounding).
    pub fn determinant(&self) -> f64 {
        Matrix3::from(self.0).determinant()
    }

    fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        self.0
            .map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
    }

    fn is_identity(&self) -> bool {
        (0..3).all(|i| {
            (0..3).all(|j| {
                let expected = if i == j { 1.0 } else { 0.0 };
                (self.0[i][j] - expected).abs() <= ORIENTATION_TOLERANCE
            })
        })
    }
}

/// How the cell sits in Cartesian space (design §4.2): the basis vectors
/// are `A = R·A_std`, where `A_std` is [`CellParams::standard_matrix`].
/// Distances, angles, the metric tensor and the volume do not depend on it;
/// Cartesian positions do.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Orientation {
    /// R = I: a along x, b in the xy plane, right-handed.
    #[default]
    Standard,
    /// The orientation of basis vectors read from a file (POSCAR) or kept
    /// through a change of cell.
    Explicit(Rotation),
}

impl Orientation {
    /// The orientation of a rotation: `Standard` when it is the identity
    /// within rounding.
    pub fn new(rotation: Rotation) -> Self {
        if rotation.is_identity() {
            Self::Standard
        } else {
            Self::Explicit(rotation)
        }
    }

    pub fn rotation(&self) -> Option<&Rotation> {
        match self {
            Self::Standard => None,
            Self::Explicit(rotation) => Some(rotation),
        }
    }

    /// Whether the basis vectors form a left-handed set (det R = −1).
    pub fn is_left_handed(&self) -> bool {
        self.rotation().is_some_and(|r| r.determinant() < 0.0)
    }
}

/// The crystal lattice: cell parameters and an orientation (design §4.2).
/// The parameters keep 90° exactly 90°, the orientation keeps the basis
/// vectors of a file that gives vectors instead of parameters.
///
/// Serialized as its cell parameters, plus the rows of R under
/// `orientation` when the orientation is explicit (project format §6).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "LatticeForm", into = "LatticeForm")]
pub struct Lattice {
    params: CellParams,
    orientation: Orientation,
}

impl Lattice {
    /// A lattice in the standard orientation.
    pub fn new(params: CellParams) -> Self {
        Self::with_orientation(params, Orientation::Standard)
    }

    pub fn with_orientation(params: CellParams, orientation: Orientation) -> Self {
        Self {
            params,
            orientation,
        }
    }

    /// The lattice with these basis vectors in Å (`basis[i]` is a, b, c), as
    /// a file such as POSCAR gives it: the cell parameters from the metric
    /// tensor ([`CellParams::from_metric`]) and the orientation that turns
    /// the standard basis onto these vectors. `None` when the vectors span
    /// no cell.
    pub fn from_matrix(basis: [[f64; 3]; 3]) -> Option<Self> {
        let a = Matrix3::from(basis);
        let g = a.transpose() * a;
        let params = CellParams::from_metric([0, 1, 2].map(|i| [0, 1, 2].map(|j| g[(i, j)])));
        if !params.is_valid() {
            return None;
        }
        Self::oriented_as(params, basis)
    }

    /// The lattice with these parameters, turned so that its basis vectors
    /// lie as close as possible to `basis`, which has the same parameters
    /// within rounding. `None` when the parameters describe no cell.
    pub(crate) fn oriented_as(params: CellParams, basis: [[f64; 3]; 3]) -> Option<Self> {
        let standard = Matrix3::from(params.standard_matrix()).try_inverse()?;
        // Rounding in the parameters leaves R = A·A_std⁻¹ slightly off
        // orthogonal; the nearest orthogonal matrix is U·Vᵀ of its singular
        // value decomposition.
        let svd = (Matrix3::from(basis) * standard).svd(true, true);
        let r = svd.u? * svd.v_t?;
        let rotation = Rotation::new([0, 1, 2].map(|i| [0, 1, 2].map(|j| r[(i, j)])))?;
        Some(Self::with_orientation(params, Orientation::new(rotation)))
    }

    pub fn params(&self) -> &CellParams {
        &self.params
    }

    /// Replaces the cell parameters, keeping the orientation.
    pub fn set_params(&mut self, params: CellParams) {
        self.params = params;
    }

    pub fn orientation(&self) -> &Orientation {
        &self.orientation
    }

    /// Whether a, b, c form a left-handed set, which only a file that gives
    /// basis vectors can do.
    pub fn is_left_handed(&self) -> bool {
        self.orientation.is_left_handed()
    }

    /// Basis vectors a, b, c in Å: `m[i]` is the i-th basis vector, the
    /// i-th column of `A = R·A_std`.
    pub fn matrix(&self) -> [[f64; 3]; 3] {
        let standard = self.params.standard_matrix();
        match &self.orientation {
            Orientation::Standard => standard,
            Orientation::Explicit(rotation) => standard.map(|basis| rotation.apply(basis)),
        }
    }

    /// Converts fractional coordinates to Cartesian Å.
    pub fn to_cartesian(&self, fractional: [f64; 3]) -> [f64; 3] {
        let m = self.matrix();
        let mut r = [0.0; 3];
        for (axis, basis) in m.iter().enumerate() {
            for (i, component) in basis.iter().enumerate() {
                r[i] += fractional[axis] * component;
            }
        }
        r
    }

    /// Converts Cartesian Å to fractional coordinates; not finite for a
    /// cell that does not exist.
    pub fn to_fractional(&self, cartesian: [f64; 3]) -> [f64; 3] {
        let inverse = Matrix3::from(self.matrix())
            .try_inverse()
            .unwrap_or_else(|| Matrix3::from_element(f64::NAN));
        let x = inverse * nalgebra::Vector3::from(cartesian);
        [x[0], x[1], x[2]]
    }
}

/// The serialized form of a [`Lattice`]: the six parameters, then the rows
/// of R for an explicit orientation only.
#[derive(Serialize, Deserialize)]
struct LatticeForm {
    a: Measured,
    b: Measured,
    c: Measured,
    alpha: Measured,
    beta: Measured,
    gamma: Measured,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    orientation: Option<[[f64; 3]; 3]>,
}

impl From<Lattice> for LatticeForm {
    fn from(lattice: Lattice) -> Self {
        let p = lattice.params;
        Self {
            a: p.a,
            b: p.b,
            c: p.c,
            alpha: p.alpha,
            beta: p.beta,
            gamma: p.gamma,
            orientation: lattice.orientation.rotation().map(|r| *r.rows()),
        }
    }
}

impl TryFrom<LatticeForm> for Lattice {
    type Error = &'static str;

    fn try_from(form: LatticeForm) -> Result<Self, Self::Error> {
        let orientation = match form.orientation {
            None => Orientation::Standard,
            Some(rows) => Orientation::new(
                Rotation::new(rows).ok_or("the orientation is not an orthogonal matrix")?,
            ),
        };
        let params = CellParams {
            a: form.a,
            b: form.b,
            c: form.c,
            alpha: form.alpha,
            beta: form.beta,
            gamma: form.gamma,
        };
        Ok(Self::with_orientation(params, orientation))
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
        let m = CellParams::new(3.0, 4.0, 5.0, 90.0, 90.0, 90.0).standard_matrix();
        assert_eq!(m, [[3.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 5.0]]);
    }

    #[test]
    fn hexagonal_b_axis_is_exact() {
        let m = CellParams::new(3.0, 3.0, 5.0, 90.0, 90.0, 120.0).standard_matrix();
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

    fn close_vectors(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> bool {
        (0..3).all(|i| (0..3).all(|j| (a[i][j] - b[i][j]).abs() < 1e-12))
    }

    /// A monoclinic cell turned by 30° about z and tilted by 20° about x.
    fn turned() -> [[f64; 3]; 3] {
        let (s, c) = 30f64.to_radians().sin_cos();
        let (t, d) = 20f64.to_radians().sin_cos();
        let rz = [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]];
        let rx = [[1.0, 0.0, 0.0], [0.0, d, -t], [0.0, t, d]];
        let rotation = Rotation::new(
            [0, 1, 2].map(|i| [0, 1, 2].map(|j| (0..3).map(|k| rx[i][k] * rz[k][j]).sum())),
        )
        .unwrap();
        CellParams::new(5.0, 6.0, 7.0, 90.0, 104.5, 90.0)
            .standard_matrix()
            .map(|v| rotation.apply(v))
    }

    #[test]
    fn vectors_in_the_standard_orientation_read_as_standard() {
        let cubic =
            Lattice::from_matrix([[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]]).unwrap();
        assert_eq!(
            cubic,
            Lattice::new(CellParams::new(4.0, 4.0, 4.0, 90.0, 90.0, 90.0))
        );
        // Hexagonal with b written to ten decimals: the angles stay exact.
        let hexagonal =
            Lattice::from_matrix([[3.0, 0.0, 0.0], [-1.5, 2.5980762114, 0.0], [0.0, 0.0, 5.0]])
                .unwrap();
        assert_eq!(hexagonal.orientation(), &Orientation::Standard);
        let params = hexagonal.params();
        assert_eq!(params.alpha.value(), 90.0);
        assert_eq!(params.beta.value(), 90.0);
        assert!((params.gamma.value() - 120.0).abs() < 1e-8);
        assert!((params.b.value() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn turned_vectors_keep_their_orientation() {
        let basis = turned();
        let lattice = Lattice::from_matrix(basis).unwrap();
        let rotation = lattice.orientation().rotation().unwrap();
        assert!((rotation.determinant() - 1.0).abs() < 1e-12);
        assert!(!lattice.is_left_handed());
        let params = lattice.params();
        assert!((params.a.value() - 5.0).abs() < 1e-12);
        assert!((params.c.value() - 7.0).abs() < 1e-12);
        assert_eq!(params.alpha.value(), 90.0);
        assert_eq!(params.gamma.value(), 90.0);
        assert!((params.beta.value() - 104.5).abs() < 1e-9);
        assert!(close_vectors(lattice.matrix(), basis));
        // Cartesian positions follow the vectors; fractional ones come back.
        let x = [0.1, 0.25, 0.7];
        let r = lattice.to_cartesian(x);
        let expected = [0, 1, 2].map(|i| (0..3).map(|axis| x[axis] * basis[axis][i]).sum::<f64>());
        assert!((0..3).all(|i| (r[i] - expected[i]).abs() < 1e-12));
        let back = lattice.to_fractional(r);
        assert!((0..3).all(|i| (back[i] - x[i]).abs() < 1e-12));
        // Changing the parameters keeps the orientation.
        let mut longer = lattice;
        longer.set_params(CellParams::new(10.0, 6.0, 7.0, 90.0, 104.5, 90.0));
        assert_eq!(longer.orientation(), lattice.orientation());
        let a = longer.matrix()[0];
        assert!((0..3).all(|i| (a[i] - 2.0 * basis[0][i]).abs() < 1e-12));
    }

    #[test]
    fn left_handed_vectors_are_kept() {
        let mut basis = turned();
        basis.swap(0, 1);
        let lattice = Lattice::from_matrix(basis).unwrap();
        assert!(lattice.is_left_handed());
        assert!((lattice.orientation().rotation().unwrap().determinant() + 1.0).abs() < 1e-12);
        assert!(close_vectors(lattice.matrix(), basis));
        // The parameters are those of the vectors in their new order.
        assert!((lattice.params().a.value() - 6.0).abs() < 1e-12);
        assert!((lattice.params().alpha.value() - 104.5).abs() < 1e-9);
    }

    #[test]
    fn vectors_that_span_no_cell_are_refused() {
        assert_eq!(
            Lattice::from_matrix([[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
            None
        );
        assert_eq!(Lattice::from_matrix([[0.0; 3]; 3]), None);
        assert_eq!(
            Lattice::from_matrix([[1.0, 0.0, 0.0], [0.0, f64::NAN, 0.0], [0.0, 0.0, 1.0]]),
            None
        );
    }

    #[test]
    fn rotations_must_be_orthogonal() {
        assert!(Rotation::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]]).is_some());
        assert!(Rotation::new([[1.0, 0.1, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
        assert!(Rotation::new([[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
        let identity = Rotation::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert_eq!(Orientation::new(identity.unwrap()), Orientation::Standard);
    }

    #[test]
    fn serialized_form() {
        let standard = Lattice::new(CellParams::new(4.0, 4.0, 4.0, 90.0, 90.0, 90.0));
        let json = serde_json::to_value(standard).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "a": {"value": 4.0}, "b": {"value": 4.0}, "c": {"value": 4.0},
                "alpha": {"value": 90.0}, "beta": {"value": 90.0}, "gamma": {"value": 90.0}
            })
        );
        let mut basis = turned();
        basis.swap(0, 1);
        let explicit = Lattice::from_matrix(basis).unwrap();
        let json = serde_json::to_value(explicit).unwrap();
        let rows = *explicit.orientation().rotation().unwrap().rows();
        assert_eq!(json["orientation"], serde_json::json!(rows));
        assert_eq!(
            serde_json::from_value::<Lattice>(json.clone()).unwrap(),
            explicit
        );
        // An orientation that is no rotation is refused.
        let mut broken = json;
        broken["orientation"][0][0] = 2.0.into();
        let error = serde_json::from_value::<Lattice>(broken).unwrap_err();
        assert!(error.to_string().contains("orthogonal"), "{error}");
    }
}
