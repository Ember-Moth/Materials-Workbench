//! Changes of cell (design §7.8) in the convention of International Tables
//! A: the new basis is `A' = A·P` and the new origin lies at `p` in the old
//! basis, so coordinates become `x' = P⁻¹·(x − p)`. P and p are exact
//! fractions. The presets (primitive and conventional cell, Niggli
//! reduction, another tabulated setting) come from moyo, through
//! `tables.rs`.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::tables::{self, Transform};
use crate::{Rational, Structure, SymOp, constraints};

/// Exact fractions with checked 128-bit arithmetic, for the matrix algebra
/// of a change of cell. A transformation whose numbers overflow it is far
/// beyond any use, and is refused rather than computed wrongly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Exact {
    num: i128,
    den: i128,
}

pub(crate) type Matrix = [[Exact; 3]; 3];
pub(crate) type Vector = [Exact; 3];

impl Exact {
    pub const ZERO: Exact = Exact { num: 0, den: 1 };
    pub const ONE: Exact = Exact { num: 1, den: 1 };

    pub fn new(num: i128, den: i128) -> Option<Self> {
        if den == 0 {
            return None;
        }
        let divisor = gcd(num, den).max(1) * den.signum();
        Some(Self {
            num: num / divisor,
            den: den / divisor,
        })
    }

    pub fn integer(value: i128) -> Self {
        Self { num: value, den: 1 }
    }

    pub fn of(value: Rational) -> Self {
        Self {
            num: value.numerator().into(),
            den: value.denominator().into(),
        }
    }

    pub fn add(self, other: Self) -> Option<Self> {
        let num = self
            .num
            .checked_mul(other.den)?
            .checked_add(other.num.checked_mul(self.den)?)?;
        Self::new(num, self.den.checked_mul(other.den)?)
    }

    pub fn sub(self, other: Self) -> Option<Self> {
        self.add(Self {
            num: other.num.checked_neg()?,
            den: other.den,
        })
    }

    pub fn mul(self, other: Self) -> Option<Self> {
        Self::new(
            self.num.checked_mul(other.num)?,
            self.den.checked_mul(other.den)?,
        )
    }

    pub fn div(self, other: Self) -> Option<Self> {
        Self::new(
            self.num.checked_mul(other.den)?,
            self.den.checked_mul(other.num)?,
        )
    }

    pub fn is_zero(self) -> bool {
        self.num == 0
    }

    pub fn is_integer(self) -> bool {
        self.den == 1
    }

    pub fn is_positive(self) -> bool {
        self.num > 0
    }

    /// The same value modulo 1, in [0, 1).
    pub fn fract(self) -> Self {
        Self {
            num: self.num.rem_euclid(self.den),
            den: self.den,
        }
    }

    /// The same value modulo 1, in (−½, ½].
    fn centered(self) -> Option<Self> {
        let f = self.fract();
        if f.num * 2 > f.den {
            f.sub(Self::ONE)
        } else {
            Some(f)
        }
    }

    pub fn to_rational(self) -> Option<Rational> {
        Some(Rational::new(
            i64::try_from(self.num).ok()?,
            i64::try_from(self.den).ok()?,
        ))
    }

    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

pub(crate) fn mat_mul(a: &Matrix, b: &Matrix) -> Option<Matrix> {
    let mut out = [[Exact::ZERO; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            for k in 0..3 {
                out[i][j] = out[i][j].add(a[i][k].mul(b[k][j])?)?;
            }
        }
    }
    Some(out)
}

pub(crate) fn mat_apply(m: &Matrix, v: &Vector) -> Option<Vector> {
    let mut out = [Exact::ZERO; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i] = out[i].add(m[i][j].mul(v[j])?)?;
        }
    }
    Some(out)
}

fn vec_sub(a: &Vector, b: &Vector) -> Option<Vector> {
    Some([a[0].sub(b[0])?, a[1].sub(b[1])?, a[2].sub(b[2])?])
}

fn determinant(m: &Matrix) -> Option<Exact> {
    let minor = |r0: usize, r1: usize, c0: usize, c1: usize| {
        m[r0][c0].mul(m[r1][c1])?.sub(m[r0][c1].mul(m[r1][c0])?)
    };
    m[0][0]
        .mul(minor(1, 2, 1, 2)?)?
        .sub(m[0][1].mul(minor(1, 2, 0, 2)?)?)?
        .add(m[0][2].mul(minor(1, 2, 0, 1)?)?)
}

/// `m⁻¹ = adj(m) / det(m)`; `None` for a singular matrix or on overflow.
pub(crate) fn mat_inverse(m: &Matrix) -> Option<Matrix> {
    let det = determinant(m)?;
    if det.is_zero() {
        return None;
    }
    let mut out = [[Exact::ZERO; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, entry) in row.iter_mut().enumerate() {
            // Cofactor of (j, i).
            let (r0, r1) = ((j + 1) % 3, (j + 2) % 3);
            let (c0, c1) = ((i + 1) % 3, (i + 2) % 3);
            let cofactor = m[r0][c0].mul(m[r1][c1])?.sub(m[r0][c1].mul(m[r1][c0])?)?;
            *entry = cofactor.div(det)?;
        }
    }
    Some(out)
}

/// How much a change of cell may scale the cell volume, either way. Larger
/// cells are supercells, which are new structures (design §7.9).
pub const MAX_VOLUME_FACTOR: i64 = 8;

/// A change of cell `(P, p)` (design §7.8): the new basis vectors are the
/// columns of P in the old basis (`matrix[i][j]` is the entry in row i,
/// column j, as for supercells), and the new origin is `origin` in the old
/// basis. Its determinant is positive, so the basis keeps its handedness,
/// and the volume changes at most [`MAX_VOLUME_FACTOR`]-fold.
///
/// Serialized as `{"matrix": [["1/2", "1/2", "0"], …], "origin": ["0", "0",
/// "1/4"]}`, the fractions as text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "TransformForm", into = "TransformForm")]
pub struct CellTransform {
    matrix: [[Rational; 3]; 3],
    origin: [Rational; 3],
}

#[derive(Serialize, Deserialize)]
struct TransformForm {
    matrix: [[Rational; 3]; 3],
    #[serde(default = "zero_origin")]
    origin: [Rational; 3],
}

fn zero_origin() -> [Rational; 3] {
    [Rational::ZERO; 3]
}

impl From<CellTransform> for TransformForm {
    fn from(transform: CellTransform) -> Self {
        Self {
            matrix: transform.matrix,
            origin: transform.origin,
        }
    }
}

impl TryFrom<TransformForm> for CellTransform {
    type Error = TransformError;

    fn try_from(form: TransformForm) -> Result<Self, Self::Error> {
        Self::new(form.matrix, form.origin)
    }
}

impl CellTransform {
    pub const IDENTITY: CellTransform = CellTransform {
        matrix: [
            [Rational::ONE, Rational::ZERO, Rational::ZERO],
            [Rational::ZERO, Rational::ONE, Rational::ZERO],
            [Rational::ZERO, Rational::ZERO, Rational::ONE],
        ],
        origin: [Rational::ZERO; 3],
    };

    /// Checks that P has a positive determinant within the volume limit.
    pub fn new(matrix: [[Rational; 3]; 3], origin: [Rational; 3]) -> Result<Self, TransformError> {
        let transform = Self { matrix, origin };
        let det = determinant(&transform.exact_matrix()).ok_or(TransformError::TooLarge)?;
        let determinant = det.to_rational().ok_or(TransformError::TooLarge)?;
        if !det.is_positive() {
            return Err(TransformError::NotPositive { determinant });
        }
        let limit = Exact::integer(MAX_VOLUME_FACTOR.into());
        let too_large = det
            .sub(limit)
            .ok_or(TransformError::TooLarge)?
            .is_positive();
        let too_small = Exact::ONE
            .sub(det.mul(limit).ok_or(TransformError::TooLarge)?)
            .ok_or(TransformError::TooLarge)?
            .is_positive();
        if too_large || too_small {
            return Err(TransformError::Volume { determinant });
        }
        Ok(transform)
    }

    /// A pure origin shift: the new origin at `origin` in the old basis.
    pub fn shift(origin: [Rational; 3]) -> Self {
        Self {
            origin,
            ..Self::IDENTITY
        }
    }

    pub fn matrix(&self) -> &[[Rational; 3]; 3] {
        &self.matrix
    }

    pub fn origin(&self) -> &[Rational; 3] {
        &self.origin
    }

    /// det P: how many times the new cell's volume is the old one's.
    pub fn determinant(&self) -> Rational {
        determinant(&self.exact_matrix())
            .and_then(Exact::to_rational)
            .expect("checked when made")
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    pub(crate) fn exact_matrix(&self) -> Matrix {
        self.matrix.map(|row| row.map(Exact::of))
    }

    pub(crate) fn exact_origin(&self) -> Vector {
        self.origin.map(Exact::of)
    }

    pub(crate) fn from_exact(matrix: &Matrix, origin: &Vector) -> Option<Self> {
        let rational = |v: &Exact| v.to_rational();
        let mut m = [[Rational::ZERO; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] = rational(&matrix[i][j])?;
            }
        }
        let o = [
            rational(&origin[0])?,
            rational(&origin[1])?,
            rational(&origin[2])?,
        ];
        Self::new(m, o).ok()
    }

    /// The change of cell back, `(P⁻¹, −P⁻¹·p)`; `None` when its numbers
    /// are out of range.
    pub fn inverse(&self) -> Option<Self> {
        let inverse = mat_inverse(&self.exact_matrix())?;
        let shifted = mat_apply(&inverse, &self.exact_origin())?;
        let origin = vec_sub(&[Exact::ZERO; 3], &shifted)?;
        Self::from_exact(&inverse, &origin)
    }

    /// This change of cell followed by `next`, given in the new cell:
    /// `(P₁·P₂, p₁ + P₁·p₂)`.
    pub fn then(&self, next: &CellTransform) -> Option<Self> {
        let p = self.exact_matrix();
        let matrix = mat_mul(&p, &next.exact_matrix())?;
        let moved = mat_apply(&p, &next.exact_origin())?;
        let origin = self.exact_origin();
        let origin = [
            origin[0].add(moved[0])?,
            origin[1].add(moved[1])?,
            origin[2].add(moved[2])?,
        ];
        Self::from_exact(&matrix, &origin)
    }

    /// The same change with its origin moved by a lattice translation into
    /// (−½, ½] on each axis.
    fn centered(&self) -> Option<Self> {
        let origin = self.exact_origin();
        let centered = [
            origin[0].centered()?,
            origin[1].centered()?,
            origin[2].centered()?,
        ];
        Self::from_exact(&self.exact_matrix(), &centered)
    }
}

/// Writes the new basis vectors and origin as the International Tables do,
/// e.g. `a-b, a+b, c; 0, 0, 1/4`.
impl fmt::Display for CellTransform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for j in 0..3 {
            if j > 0 {
                f.write_str(", ")?;
            }
            let mut text = String::new();
            for (i, axis) in ["a", "b", "c"].iter().enumerate() {
                let c = self.matrix[i][j];
                if c.is_zero() {
                    continue;
                }
                let sign = if c.numerator() < 0 { "-" } else { "+" };
                let num = c.numerator().abs();
                let den = c.denominator();
                let magnitude = match (num, den) {
                    (1, 1) => axis.to_string(),
                    (n, 1) => format!("{n}{axis}"),
                    (1, d) => format!("{axis}/{d}"),
                    (n, d) => format!("{n}{axis}/{d}"),
                };
                text.push_str(sign);
                text.push_str(&magnitude);
            }
            f.write_str(text.strip_prefix('+').unwrap_or(&text))?;
        }
        f.write_str("; ")?;
        for (i, value) in self.origin.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{value}")?;
        }
        Ok(())
    }
}

/// Why a change of cell cannot be applied (design §7.8). The structure is
/// unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransformError {
    /// det P is zero (the vectors span no cell) or negative (they are
    /// left-handed).
    NotPositive { determinant: Rational },
    /// The cell would grow or shrink more than [`MAX_VOLUME_FACTOR`]-fold.
    Volume { determinant: Rational },
    /// The numbers are too large to compute with exactly, or the new cell
    /// too skewed to place.
    TooLarge,
    /// The new basis vector `vector` (0 for a′, 1 for b′, 2 for c′) is not a
    /// lattice translation of the structure: the new cell is smaller than
    /// its lattice allows.
    NotATranslation { vector: usize },
    /// The operation does not map the new lattice onto itself, so the
    /// symmetry has no description in the new cell; for example a 2a × a ×
    /// c cell of a tetragonal structure.
    BreaksSymmetry { operation: SymOp },
}

impl TransformError {
    /// A stable identifier for programs and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotPositive { .. } => "transform_not_positive",
            Self::Volume { .. } => "transform_volume",
            Self::TooLarge => "transform_too_large",
            Self::NotATranslation { .. } => "transform_not_a_translation",
            Self::BreaksSymmetry { .. } => "transform_breaks_symmetry",
        }
    }
}

/// English, for programs, logs and AI callers.
impl fmt::Display for TransformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPositive { determinant } => write!(
                f,
                "the transformation matrix has determinant {determinant}; it must be positive"
            ),
            Self::Volume { determinant } => write!(
                f,
                "the transformation matrix has determinant {determinant}; a change of cell may \
                 scale the volume at most {MAX_VOLUME_FACTOR}-fold either way (make a supercell \
                 for larger cells)"
            ),
            Self::TooLarge => f.write_str("the transformation's numbers are too large"),
            Self::NotATranslation { vector } => write!(
                f,
                "the new basis vector {} is not a lattice translation of the structure",
                ["a'", "b'", "c'"][*vector]
            ),
            Self::BreaksSymmetry { operation } => write!(
                f,
                "the symmetry operation {operation} does not map the new lattice onto itself"
            ),
        }
    }
}

impl std::error::Error for TransformError {}

/// A change of cell that moyo provides for a structure (design §7.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "preset", rename_all = "snake_case")]
pub enum CellPreset {
    /// The primitive cell: the conventional cell's centering removed.
    Primitive,
    /// The conventional cell of the standard setting.
    Conventional,
    /// The Niggli-reduced primitive cell.
    Niggli,
    /// The conventional cell of another tabulated setting of the same type:
    /// another unique axis, cell choice, axis order or origin choice.
    Setting { hall_number: u16 },
}

/// A tabulated setting of the structure's space group type.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SettingChoice {
    pub hall_number: u16,
    /// International Tables number of the type, 1–230.
    pub number: u16,
    /// Full Hermann–Mauguin symbol, e.g. `P 1 21/n 1`.
    pub hm_symbol: &'static str,
    /// Unique axis and cell choice, axes, or origin choice, e.g. `b2`.
    pub code: &'static str,
}

/// The tabulated settings of the structure's space group type, other than
/// the one its operations are in.
pub fn other_settings(structure: &Structure) -> Vec<SettingChoice> {
    let Some(number) = type_number(structure) else {
        return Vec::new();
    };
    tables::settings_of(number)
        .filter(|s| !tables::same_operations(&s.symmetry, &structure.symmetry))
        .map(SettingChoice::of)
        .collect()
}

/// The tabulated setting a Hermann–Mauguin symbol names, e.g. `P 1 1 21/a`
/// or `F d -3 m :1` (a short symbol names the standard setting).
pub fn find_setting(symbol: &str) -> Option<SettingChoice> {
    tables::find_hm(symbol).map(SettingChoice::of)
}

impl SettingChoice {
    fn of(setting: &tables::Setting) -> Self {
        Self {
            hall_number: setting.hall_number,
            number: setting.number,
            hm_symbol: setting.hm_full,
            code: setting.code,
        }
    }
}

fn type_number(structure: &Structure) -> Option<u16> {
    if let Some(setting) = tables::match_operations(&structure.symmetry) {
        return Some(setting.number);
    }
    let (setting, _) = tables::search_operations(&structure.symmetry, metric(structure))?;
    Some(setting.number)
}

/// The metric the operations preserve nearest to the cell's.
fn metric(structure: &Structure) -> [[f64; 3]; 3] {
    constraints::symmetrized_metric(&structure.symmetry, structure.lattice.params().metric())
}

impl CellPreset {
    /// The change of cell this preset makes for `structure`: the identity
    /// when it is in that cell already, `None` when moyo provides none (or
    /// the setting is of another type).
    ///
    /// A change of cell composed with an operation of the structure gives
    /// the same new operations; of those, this picks the one whose basis
    /// lies closest to the standard orientation, then the one with the
    /// shortest origin shift. So the conventional cell of a primitive cell
    /// made from a conventional cell in the standard orientation is that
    /// cell again.
    pub fn transform(self, structure: &Structure) -> Option<CellTransform> {
        let symmetry = &structure.symmetry;
        let tabulated = tables::match_operations(symmetry);
        match self {
            Self::Primitive => {
                if !is_centered(structure) {
                    return Some(CellTransform::IDENTITY);
                }
                match tabulated {
                    Some(setting) => exact(tables::primitive_basis(setting.hall_number), [0.0; 3]),
                    None => {
                        let (_, primitive) =
                            tables::standardize(symmetry, metric(structure), None)?;
                        let primitive = from_moyo(&primitive)?;
                        canonical(
                            structure,
                            primitive,
                            &operations(structure, &primitive)?,
                            false,
                        )
                    }
                }
            }
            Self::Conventional => {
                if tabulated.is_some_and(|s| s.is_standard()) {
                    return Some(CellTransform::IDENTITY);
                }
                let (conventional, _) = tables::standardize(symmetry, metric(structure), None)?;
                let number = type_number(structure)?;
                canonical(
                    structure,
                    from_moyo(&conventional)?,
                    &tables::standard(number).symmetry,
                    true,
                )
            }
            Self::Setting { hall_number } => {
                let target = (1..=530)
                    .contains(&hall_number)
                    .then(|| tables::setting(hall_number))?;
                if tables::same_operations(&target.symmetry, symmetry) {
                    return Some(CellTransform::IDENTITY);
                }
                if type_number(structure) != Some(target.number) {
                    return None;
                }
                // moyo standardizes into one setting at a time and cannot
                // turn a unique axis; going through the standard setting
                // reaches every setting.
                let (to_standard, _) = tables::standardize(symmetry, metric(structure), None)?;
                let target_metric = constraints::symmetrized_metric(
                    &target.symmetry,
                    structure.lattice.params().metric(),
                );
                let (from_target, _) = tables::standardize(&target.symmetry, target_metric, None)?;
                let composite =
                    from_moyo(&to_standard)?.then(&from_moyo(&from_target)?.inverse()?)?;
                canonical(structure, composite, &target.symmetry, true)
            }
            Self::Niggli => {
                let primitive = Self::Primitive.transform(structure)?;
                // The Niggli cell of the primitive lattice, found on a
                // right-handed basis with the same metric.
                let basis =
                    transformed_basis(&structure.lattice.params().standard_matrix(), &primitive);
                let t = tables::niggli(basis)?;
                let reduction = t.map(|row| row.map(|v| Exact::integer(v.into())));
                if primitive.is_identity() && reduction == identity() {
                    return Some(CellTransform::IDENTITY);
                }
                let matrix = mat_mul(&primitive.exact_matrix(), &reduction)?;
                let transform = CellTransform::from_exact(&matrix, &primitive.exact_origin())?;
                canonical(
                    structure,
                    transform,
                    &operations(structure, &transform)?,
                    false,
                )
            }
        }
    }
}

fn identity() -> Matrix {
    CellTransform::IDENTITY.exact_matrix()
}

/// Whether the operations include a pure translation other than the
/// identity: a centered cell.
fn is_centered(structure: &Structure) -> bool {
    structure
        .symmetry
        .operations()
        .iter()
        .any(|op| !op.is_identity() && op.rotation() == SymOp::IDENTITY.rotation())
}

/// The basis vectors (rows) of `A·P` for basis rows `basis`.
fn transformed_basis(basis: &[[f64; 3]; 3], transform: &CellTransform) -> [[f64; 3]; 3] {
    let p = transform.exact_matrix();
    [0, 1, 2].map(|j| [0, 1, 2].map(|k| (0..3).map(|i| p[i][j].to_f64() * basis[i][k]).sum()))
}

/// moyo's floating-point change of cell as fractions; `None` when an entry
/// is no fraction with a denominator dividing 24.
fn from_moyo(transform: &Transform) -> Option<CellTransform> {
    exact(transform.linear, transform.origin)
}

fn exact(matrix: [[f64; 3]; 3], origin: [f64; 3]) -> Option<CellTransform> {
    let mut m = [[Rational::ZERO; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = Rational::from_decimal(matrix[i][j])?;
        }
    }
    let o = [
        Rational::from_decimal(origin[0])?,
        Rational::from_decimal(origin[1])?,
        Rational::from_decimal(origin[2])?,
    ];
    CellTransform::new(m, o).ok()?.centered()
}

fn operations(structure: &Structure, transform: &CellTransform) -> Option<crate::Symmetry> {
    crate::transform::transform_operations(&structure.symmetry, transform).ok()
}

/// Whether the change of cell turns the structure's operations into
/// exactly `target`.
fn reaches(structure: &Structure, transform: &CellTransform, target: &crate::Symmetry) -> bool {
    crate::transform::transform_operations(&structure.symmetry, transform)
        .is_ok_and(|symmetry| tables::same_operations(&symmetry, target))
}

/// The rotations W (det +1) whose change of cell `W·P` may give the same
/// new operations as `P`: the proper rotations of the operations, and the
/// symmetries of the lattice itself (integer matrices with entries −1, 0,
/// 1 that keep the metric), which can relate two standard cells of a
/// structure whose lattice has more symmetry than its operations, such as
/// the two directions of a polar axis.
fn rotations(structure: &Structure) -> Vec<[[i32; 3]; 3]> {
    let mut rotations: Vec<[[i32; 3]; 3]> = structure
        .symmetry
        .operations()
        .iter()
        .filter(|op| op.determinant() == 1)
        .map(|op| *op.rotation())
        .collect();
    let g = structure.lattice.params().metric();
    let tolerance = 1e-6 * (0..3).map(|i| g[i][i]).fold(0.0, f64::max);
    let dot = |u: &[i32; 3], v: &[i32; 3]| {
        let mut sum = 0.0;
        for i in 0..3 {
            for j in 0..3 {
                sum += f64::from(u[i]) * g[i][j] * f64::from(v[j]);
            }
        }
        sum
    };
    let vectors: Vec<[i32; 3]> = (0..27)
        .map(|n| [n % 3 - 1, n / 3 % 3 - 1, n / 9 - 1])
        .filter(|v| *v != [0; 3])
        .collect();
    // Column j of W is the image of basis vector j: it keeps its length
    // and its angles to the others.
    let keeps =
        |u: &[i32; 3], v: &[i32; 3], i: usize, j: usize| (dot(u, v) - g[i][j]).abs() < tolerance;
    for c0 in vectors.iter().filter(|v| keeps(v, v, 0, 0)) {
        for c1 in vectors.iter().filter(|v| keeps(v, v, 1, 1)) {
            if !keeps(c0, c1, 0, 1) {
                continue;
            }
            for c2 in vectors.iter().filter(|v| keeps(v, v, 2, 2)) {
                if !keeps(c0, c2, 0, 2) || !keeps(c1, c2, 1, 2) {
                    continue;
                }
                let w = [0, 1, 2].map(|i| [c0[i], c1[i], c2[i]]);
                let det = SymOp::new(w, [Rational::ZERO; 3]).determinant();
                if det == 1 && !rotations.contains(&w) {
                    rotations.push(w);
                }
            }
        }
    }
    rotations
}

/// Among the changes of cell that turn the operations into `target`, the
/// one moyo's `transform` stands for (design §7.8):
///
/// 1. the linear part: `W·P` for W from [`rotations`], whose new basis lies
///    closest to the standard orientation; with `flips`, the new axes may
///    also be reversed in pairs, and cells with more non-acute angles come
///    first, as conventional cells have them (β ≥ 90° for unique axis b, γ
///    = 120° for hexagonal axes);
/// 2. the origin: the shortest shift, so the old origin when it gives the
///    same operations, then the one that puts the sites' coordinates
///    nearest the origin (wrapped into [0, 1)), as files usually give
///    them. The origin is searched among the multiples of 1/8 and 1/6 that
///    keep the new operations, which covers the translations of every
///    space group's normalizer; along a polar axis, where any shift keeps
///    them, it is 0.
pub(crate) fn canonical(
    structure: &Structure,
    transform: CellTransform,
    target: &crate::Symmetry,
    flips: bool,
) -> Option<CellTransform> {
    let linear = linear_part(structure, &transform, target, flips)?;
    origin(structure, &linear, target)
}

/// Step 1 of [`canonical`]: a change of cell with the chosen linear part and
/// some origin that reaches `target`.
fn linear_part(
    structure: &Structure,
    transform: &CellTransform,
    target: &crate::Symmetry,
    flips: bool,
) -> Option<CellTransform> {
    let lattice = &structure.lattice;
    let basis = lattice.matrix();
    let origin = transform.exact_origin();
    let integer = |m: &[[i32; 3]; 3]| m.map(|row| row.map(|v| Exact::integer(v.into())));
    let signs: &[[i32; 3]] = if flips {
        &[[1, 1, 1], [-1, -1, 1], [-1, 1, -1], [1, -1, -1]]
    } else {
        &[[1, 1, 1]]
    };
    let mut candidates: Vec<((usize, i64), Vec<CellTransform>)> = Vec::new();
    for sign in signs {
        let flip = integer(&[0, 1, 2].map(|i| [0, 1, 2].map(|j| if i == j { sign[i] } else { 0 })));
        let matrix = mat_mul(&transform.exact_matrix(), &flip)?;
        let flipped = CellTransform::from_exact(&matrix, &origin)?;
        let params = crate::transform::transformed_params(lattice.params(), &flipped);
        let non_acute = if flips {
            [params.alpha, params.beta, params.gamma]
                .iter()
                .filter(|angle| angle.value() >= 90.0 - 1e-9)
                .count()
        } else {
            0
        };
        let standard = nalgebra::Matrix3::from(params.standard_matrix()).try_inverse()?;
        for w in rotations(structure) {
            let exact_w = integer(&w);
            let candidate_matrix = mat_mul(&exact_w, &matrix)?;
            let moved = mat_apply(&exact_w, &origin)?;
            let mut origins = vec![moved];
            for op in structure.symmetry.operations() {
                if *op.rotation() == w {
                    let t = op.translation().map(Exact::of);
                    origins.push([
                        moved[0].add(t[0])?,
                        moved[1].add(t[1])?,
                        moved[2].add(t[2])?,
                    ]);
                }
            }
            let transforms: Vec<CellTransform> = origins
                .iter()
                .filter_map(|o| CellTransform::from_exact(&candidate_matrix, o))
                .collect();
            let Some(first) = transforms.first() else {
                continue;
            };
            // trace(R) = 1 + 2 cos θ for the rotation R that turns the
            // standard orientation of the new parameters onto the new basis.
            let new_basis = transformed_basis(&basis, first);
            let r = nalgebra::Matrix3::from(new_basis) * standard;
            let closeness = (r.trace() * 1e9).round() as i64;
            candidates.push(((non_acute, closeness), transforms));
        }
    }
    // Stable: among equals, the operations' own rotations come first.
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    candidates
        .into_iter()
        .flat_map(|(_, transforms)| transforms)
        .find(|candidate| reaches(structure, candidate, target))
}

/// Step 2 of [`canonical`]: the origin for the linear part of `transform`,
/// which reaches `target`.
fn origin(
    structure: &Structure,
    transform: &CellTransform,
    target: &crate::Symmetry,
) -> Option<CellTransform> {
    let matrix = transform.exact_matrix();
    let inverse = mat_inverse(&matrix)?.map(|row| row.map(Exact::to_f64));
    // Moving the origin by δ changes each new translation by P⁻¹·(W − I)·δ;
    // the new operations stay the same when that is a lattice or centering
    // translation of `target`.
    let centering: Vec<[f64; 3]> = target
        .operations()
        .iter()
        .filter(|op| op.rotation() == SymOp::IDENTITY.rotation())
        .map(|op| op.translation().map(Rational::to_f64))
        .collect();
    let mut rotations: Vec<[[i32; 3]; 3]> = Vec::new();
    for op in structure.symmetry.operations() {
        if !rotations.contains(op.rotation()) {
            rotations.push(*op.rotation());
        }
    }
    let keeps = |delta: [f64; 3]| {
        rotations.iter().all(|w| {
            let moved = [0, 1, 2]
                .map(|i| (0..3).map(|j| f64::from(w[i][j]) * delta[j]).sum::<f64>() - delta[i]);
            let change = [0, 1, 2].map(|i| (0..3).map(|j| inverse[i][j] * moved[j]).sum::<f64>());
            centering.iter().any(|t| {
                (0..3).all(|i| {
                    let d = change[i] - t[i];
                    (d - d.round()).abs() < 1e-9
                })
            })
        })
    };
    // Along a polar axis the origin is free: every operation keeps the
    // shift. Each such direction (exactly, from the null space of all
    // W − I) gets the coordinate 0.
    let rows: Vec<[Rational; 3]> = rotations
        .iter()
        .flat_map(|w| {
            (0..3).map(move |i| {
                [0, 1, 2].map(|j| Rational::integer(i64::from(w[i][j] - i32::from(i == j))))
            })
        })
        .collect();
    let free: Vec<(usize, [Exact; 3])> = crate::linalg::null_space(&rows)
        .into_iter()
        .map(|v| {
            let column = (0..3)
                .find(|&i| v[i] == Rational::ONE)
                .expect("a free column");
            (column, v.map(Exact::of))
        })
        .collect();
    let pinned = |mut o: [Exact; 3]| -> Option<[Exact; 3]> {
        for (column, v) in &free {
            let amount = o[*column];
            for i in 0..3 {
                o[i] = o[i].sub(amount.mul(v[i])?)?;
            }
        }
        Some(o)
    };
    let mut steps: Vec<Exact> = Vec::new();
    for den in [8, 6] {
        for num in 0..den {
            let step = Exact::new(num, den)?;
            if !steps.contains(&step) {
                steps.push(step);
            }
        }
    }
    let lattice = &structure.lattice;
    let coordinates = |origin: [f64; 3]| -> f64 {
        structure
            .sites
            .iter()
            .flat_map(|site| {
                let d = [0, 1, 2].map(|i| site.position[i] - origin[i]);
                [0, 1, 2].map(|i| {
                    let x: f64 = (0..3).map(|j| inverse[i][j] * d[j]).sum();
                    let wrapped = x - x.floor();
                    // Within rounding of 1 is 0.
                    if wrapped > 1.0 - 1e-9 { 0.0 } else { wrapped }
                })
            })
            .sum()
    };
    let mut candidates: Vec<(i64, f64, CellTransform)> = Vec::new();
    for &x in &steps {
        for &y in &steps {
            for &z in &steps {
                let delta = [x, y, z];
                if !keeps(delta.map(Exact::to_f64)) {
                    continue;
                }
                let o = transform.exact_origin();
                let Some(candidate) = (|| {
                    let shifted = [o[0].add(x)?, o[1].add(y)?, o[2].add(z)?];
                    CellTransform::from_exact(&matrix, &pinned(shifted)?)?.centered()
                })() else {
                    continue;
                };
                let origin = candidate.origin.map(Rational::to_f64);
                let length = lattice
                    .to_cartesian(origin)
                    .iter()
                    .map(|v| v * v)
                    .sum::<f64>();
                candidates.push((
                    (length * 1e9).round() as i64,
                    coordinates(origin),
                    candidate,
                ));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    candidates
        .into_iter()
        .map(|(_, _, candidate)| candidate)
        .find(|candidate| reaches(structure, candidate, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{self, SampleKey};
    use crate::{
        Adp, AtomFate, AtomRef, CellParams, Command, CrystalDocument, Derived, Edit, EditError,
        EditMode, Lattice, Measured, Measurement, Orientation, Rotation, Site, SiteId, Symmetry,
    };

    fn r(text: &str) -> Rational {
        Rational::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    fn transform(matrix: [[&str; 3]; 3], origin: [&str; 3]) -> CellTransform {
        CellTransform::new(matrix.map(|row| row.map(r)), origin.map(r)).unwrap()
    }

    fn apply(structure: &Structure, transform: CellTransform) -> Result<Structure, EditError> {
        Edit::TransformCell { transform }
            .apply(structure)
            .map(|applied| applied.structure)
    }

    fn rutile() -> Structure {
        sample::load(SampleKey::Rutile).structure
    }

    fn structure(hm: &str, params: CellParams, sites: Vec<Site>) -> Structure {
        Structure::new(
            Lattice::new(params),
            Symmetry::from_hm_symbol(hm).unwrap(),
            sites,
        )
    }

    fn cubic(a: f64) -> CellParams {
        CellParams::new(a, a, a, 90.0, 90.0, 90.0)
    }

    /// The atoms of the cell in Cartesian Å, with their elements.
    fn cartesian(structure: &Structure) -> Vec<(String, [f64; 3])> {
        crate::orbits(structure)
            .iter()
            .flat_map(|orbit| {
                let element = structure.site(orbit.site).unwrap().element().to_string();
                orbit
                    .positions
                    .iter()
                    .map(move |x| (element.clone(), structure.lattice.to_cartesian(*x)))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Whether every atom of each structure sits on an atom of the same
    /// element of the other, up to a lattice translation of the other,
    /// after moving `a` by `shift` Å: the same crystal in two cells, `a`
    /// with its origin `shift` further along.
    fn same_crystal(a: &Structure, b: &Structure, shift: [f64; 3]) -> bool {
        let covered = |from: &Structure, onto: &Structure, sign: f64| {
            let atoms = cartesian(onto);
            cartesian(from).iter().all(|(element, r)| {
                atoms.iter().any(|(e, s)| {
                    let d = onto
                        .lattice
                        .to_fractional([0, 1, 2].map(|i| r[i] + sign * shift[i] - s[i]));
                    e == element && d.iter().all(|v| (v - v.round()).abs() < 1e-9)
                })
            })
        };
        covered(a, b, 1.0) && covered(b, a, -1.0)
    }

    /// Cell parameters the operations preserve, rounded to 4 decimals (Å)
    /// and 2 (degrees) as a file gives them, with obtuse angles as the
    /// conventional cells of the tables have them.
    fn cell_for(symmetry: &Symmetry) -> CellParams {
        let generic = CellParams::new(5.1, 6.2, 7.3, 97.0, 98.0, 99.0);
        let g = constraints::symmetrized_metric(symmetry, generic.metric());
        let p = CellParams::from_metric(g);
        let round = |m: Measured, places: i32| {
            let scale = 10f64.powi(places);
            Measured::exact((m.value() * scale).round() / scale)
        };
        CellParams {
            a: round(p.a, 4),
            b: round(p.b, 4),
            c: round(p.c, 4),
            alpha: round(p.alpha, 2),
            beta: round(p.beta, 2),
            gamma: round(p.gamma, 2),
        }
    }

    #[test]
    fn coordinates_and_operations_follow_the_international_tables() {
        // a′ = c, b′ = a, c′ = b: x′ = (z, x, y).
        let rutile = rutile();
        let cyclic = transform(
            [["0", "1", "0"], ["0", "0", "1"], ["1", "0", "0"]],
            ["0"; 3],
        );
        let turned = apply(&rutile, cyclic).unwrap();
        assert_eq!(turned.sites[1].position, [0.0, 0.3048, 0.3048]);
        let p = turned.lattice.params();
        assert_eq!(
            [p.a.value(), p.b.value(), p.c.value()],
            [2.959, 4.594, 4.594]
        );
        // The screw axis -y+1/2,x+1/2,z+1/2 becomes x+1/2,-z+1/2,y+1/2.
        assert!(
            turned
                .symmetry
                .operations()
                .contains(&SymOp::parse("x+1/2,-z+1/2,y+1/2").unwrap())
        );
        assert_eq!(turned.symmetry.operations().len(), 16);
        assert!(same_crystal(&turned, &rutile, [0.0; 3]));
        // The same atoms, the same site ids and labels.
        assert_eq!(turned.sites.len(), 2);
        assert_eq!(turned.sites[1].id, rutile.sites[1].id);
        assert_eq!(turned.sites[1].label(), "O1");

        // A new origin at 1/2, 1/2, 1/2: x′ = x − p, w′ = w + W·p − p.
        let shifted = apply(&rutile, CellTransform::shift([r("1/2"); 3])).unwrap();
        assert_eq!(shifted.sites[0].position, [-0.5, -0.5, -0.5]);
        assert!(
            shifted
                .symmetry
                .operations()
                .contains(&SymOp::parse("-x,-y,-z").unwrap())
        );
    }

    #[test]
    fn larger_cells_gain_centering_translations() {
        let rutile = rutile();
        let double = transform(
            [["1", "0", "0"], ["0", "1", "0"], ["0", "0", "2"]],
            ["0"; 3],
        );
        let tall = apply(&rutile, double).unwrap();
        assert_eq!(tall.symmetry.operations().len(), 32);
        assert!(
            tall.symmetry
                .operations()
                .contains(&SymOp::parse("x,y,z+1/2").unwrap())
        );
        assert_eq!(tall.lattice.params().c.value(), 2.0 * 2.959);
        assert_eq!(Derived::compute(&tall).atoms_in_cell(), 12);
        assert_eq!(
            Symmetry::new(tall.symmetry.operations().to_vec()),
            Ok(tall.symmetry.clone())
        );

        // In P1 the new translation makes the structure symmetric.
        let p1 = crate::transform::expand_to_p1(&rutile).structure;
        let wide = transform(
            [["2", "0", "0"], ["0", "1", "0"], ["0", "0", "1"]],
            ["0"; 3],
        );
        let wide = apply(&p1, wide).unwrap();
        assert_eq!(wide.edit_mode(), EditMode::Symmetric);
        assert_eq!(
            wide.symmetry.operations(),
            [SymOp::IDENTITY, SymOp::parse("x+1/2,y,z").unwrap()]
        );
        assert_eq!(wide.sites[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(wide.sites[2].position, [0.1524, 0.3048, 0.0]);
    }

    #[test]
    fn smaller_cells_merge_operations() {
        // Rock salt: F m -3 m to its primitive cell.
        let nacl = structure(
            "F m -3 m",
            cubic(5.64),
            vec![
                Site::single(1, "Na1", "Na", [0.0, 0.0, 0.0]),
                Site::single(2, "Cl1", "Cl", [0.5, 0.5, 0.5]),
            ],
        );
        let primitive = CellPreset::Primitive.transform(&nacl).unwrap();
        assert_eq!(
            primitive,
            transform(
                [
                    ["0", "1/2", "1/2"],
                    ["1/2", "0", "1/2"],
                    ["1/2", "1/2", "0"]
                ],
                ["0"; 3]
            )
        );
        assert_eq!(primitive.to_string(), "b/2+c/2, a/2+c/2, a/2+b/2; 0, 0, 0");
        let small = apply(&nacl, primitive).unwrap();
        assert_eq!(small.symmetry.operations().len(), 48);
        assert_eq!(Derived::compute(&small).atoms_in_cell(), 2);
        let p = small.lattice.params();
        assert!((p.a.value() - 5.64 / 2f64.sqrt()).abs() < 1e-12);
        assert_eq!(
            [p.alpha.value(), p.beta.value(), p.gamma.value()],
            [60.0, 60.0, 60.0]
        );
        assert_eq!(small.sites[1].position, [0.5, 0.5, 0.5]);
        assert!(same_crystal(&small, &nacl, [0.0; 3]));
        assert_eq!(
            Symmetry::new(small.symmetry.operations().to_vec()),
            Ok(small.symmetry.clone())
        );
    }

    #[test]
    fn cells_smaller_than_the_lattice_are_refused() {
        let rutile = rutile();
        let half = transform(
            [["1", "0", "0"], ["0", "1", "0"], ["0", "0", "1/2"]],
            ["0"; 3],
        );
        assert_eq!(
            apply(&rutile, half),
            Err(EditError::Transform(TransformError::NotATranslation {
                vector: 2
            }))
        );
        // A 2a × a × c cell loses the fourfold axis.
        let long = transform(
            [["2", "0", "0"], ["0", "1", "0"], ["0", "0", "1"]],
            ["0"; 3],
        );
        assert!(matches!(
            apply(&rutile, long),
            Err(EditError::Transform(TransformError::BreaksSymmetry { .. }))
        ));
    }

    #[test]
    fn the_determinant_must_be_positive_and_bounded() {
        let det = |d: &str| {
            CellTransform::new(
                [
                    [r(d), r("0"), r("0")],
                    [r("0"), r("1"), r("0")],
                    [r("0"), r("0"), r("1")],
                ],
                [r("0"); 3],
            )
        };
        assert_eq!(
            det("0"),
            Err(TransformError::NotPositive {
                determinant: r("0")
            })
        );
        assert!(matches!(det("-1"), Err(TransformError::NotPositive { .. })));
        assert!(matches!(det("9"), Err(TransformError::Volume { .. })));
        assert!(matches!(det("1/9"), Err(TransformError::Volume { .. })));
        assert!(det("8").is_ok() && det("1/8").is_ok());
        let huge = Rational::new(i64::MAX, 1);
        assert_eq!(
            CellTransform::new(
                [
                    [huge, huge, r("0")],
                    [r("0"), huge, huge],
                    [huge, r("0"), huge]
                ],
                [r("0"); 3]
            ),
            Err(TransformError::TooLarge)
        );
    }

    #[test]
    fn conventional_and_primitive_cells_round_trip_exactly() {
        // Every centered standard setting, with an atom on a general
        // position and one with anisotropic displacement parameters.
        let mut checked = 0;
        for number in 1..=230 {
            let setting = tables::standard(number);
            let symmetry = &setting.symmetry;
            if !symmetry
                .operations()
                .iter()
                .any(|op| !op.is_identity() && op.rotation() == SymOp::IDENTITY.rotation())
            {
                continue;
            }
            let params = cell_for(symmetry);
            let adp =
                Adp::Anisotropic([0.011, 0.012, 0.013, 0.001, 0.002, 0.003].map(Measured::exact));
            let mut atom = Site::single(1, "Fe1", "Fe", [0.0713, 0.3291, 0.1847]);
            atom.occupants[0].adp = Some(adp);
            let original = Structure::new(
                Lattice::new(params),
                symmetry.clone(),
                vec![atom, Site::single(2, "O1", "O", [0.0, 0.0, 0.0])],
            );
            let primitive = CellPreset::Primitive.transform(&original).unwrap();
            assert!(!primitive.is_identity(), "No. {number}");
            let small = apply(&original, primitive).unwrap();
            assert_eq!(
                small.symmetry.operations().len() * tables::primitive_order(setting.hall_number),
                symmetry.operations().len(),
                "No. {number}"
            );
            let conventional = CellPreset::Conventional.transform(&small).unwrap();
            assert_eq!(conventional, primitive.inverse().unwrap(), "No. {number}");
            let back = apply(&small, conventional).unwrap();
            assert_eq!(back, original, "No. {number}");
            assert!(same_crystal(&small, &original, [0.0; 3]), "No. {number}");
            checked += 1;
        }
        assert_eq!(checked, 81);
    }

    #[test]
    fn niggli_reduction_of_a_known_cell() {
        // Křivý & Gruber (1976): A = 9, B = 27, C = 4, ξ = −5, η = −4, ζ = −22
        // reduces to A = 4, B = 9, C = 9, ξ = 9, η = 3, ζ = 4.
        let g = |a: f64, b: f64, c: f64, xi: f64, eta: f64, zeta: f64| {
            [
                [a, zeta / 2.0, eta / 2.0],
                [zeta / 2.0, b, xi / 2.0],
                [eta / 2.0, xi / 2.0, c],
            ]
        };
        let params = CellParams::from_metric(g(9.0, 27.0, 4.0, -5.0, -4.0, -22.0));
        let p1 = Structure::new(
            Lattice::new(params),
            Symmetry::p1(),
            vec![Site::single(1, "C1", "C", [0.1, 0.2, 0.3])],
        );
        let niggli = CellPreset::Niggli.transform(&p1).unwrap();
        assert_eq!(niggli.determinant(), Rational::ONE);
        let reduced = apply(&p1, niggli).unwrap();
        let metric = reduced.lattice.params().metric();
        let expected = g(4.0, 9.0, 9.0, 9.0, 3.0, 4.0);
        for (got, want) in metric.iter().flatten().zip(expected.iter().flatten()) {
            assert!((got - want).abs() < 1e-9, "{metric:?}");
        }
        assert!(same_crystal(&reduced, &p1, [0.0; 3]));
        // Reduced already.
        assert_eq!(
            CellPreset::Niggli.transform(&reduced),
            Some(CellTransform::IDENTITY)
        );

        // Face-centered cubic: the 60° rhombohedral cell of a/√2; body
        // centered: a√3/2 with angles of 109.47°.
        let fcc = structure(
            "F m -3 m",
            cubic(4.0),
            vec![Site::single(1, "Cu1", "Cu", [0.0; 3])],
        );
        let cell = apply(&fcc, CellPreset::Niggli.transform(&fcc).unwrap()).unwrap();
        let p = cell.lattice.params();
        assert!((p.a.value() - 8f64.sqrt()).abs() < 1e-12);
        assert_eq!(
            [p.alpha.value(), p.beta.value(), p.gamma.value()],
            [60.0; 3]
        );
        let bcc = structure(
            "I m -3 m",
            cubic(4.0),
            vec![Site::single(1, "Fe1", "Fe", [0.0; 3])],
        );
        let cell = apply(&bcc, CellPreset::Niggli.transform(&bcc).unwrap()).unwrap();
        let p = cell.lattice.params();
        assert!((p.a.value() - 12f64.sqrt()).abs() < 1e-12);
        let obtuse = (-1.0f64 / 3.0).acos().to_degrees();
        for angle in [p.alpha, p.beta, p.gamma] {
            assert!((angle.value() - obtuse).abs() < 1e-9, "{p:?}");
        }
    }

    #[test]
    fn every_tabulated_setting_is_reached_and_left_exactly() {
        let mut settings = 0;
        for number in 1..=230 {
            let standard = tables::standard(number);
            let original = Structure::new(
                Lattice::new(cell_for(&standard.symmetry)),
                standard.symmetry.clone(),
                vec![Site::single(1, "Fe1", "Fe", [0.0713, 0.3291, 0.1847])],
            );
            let choices = other_settings(&original);
            assert_eq!(
                choices.len() + 1,
                tables::settings_of(number).count(),
                "No. {number}"
            );
            for choice in choices {
                let preset = CellPreset::Setting {
                    hall_number: choice.hall_number,
                };
                let transform = preset
                    .transform(&original)
                    .unwrap_or_else(|| panic!("No. {number} to Hall {}", choice.hall_number));
                let other = apply(&original, transform).unwrap();
                assert!(
                    tables::same_operations(
                        &other.symmetry,
                        &tables::setting(choice.hall_number).symmetry
                    ),
                    "Hall {}",
                    choice.hall_number
                );
                assert!(same_crystal(
                    &other,
                    &original,
                    shift_of(&original, &transform)
                ));
                let back = CellPreset::Conventional.transform(&other).unwrap();
                assert_eq!(
                    apply(&other, back).unwrap(),
                    original,
                    "Hall {}",
                    choice.hall_number
                );
                settings += 1;
            }
        }
        // 530 settings, of which 230 are standard.
        assert_eq!(settings, 530 - 230);
    }

    /// The Cartesian shift of the atoms when the origin moves to p.
    fn shift_of(structure: &Structure, transform: &CellTransform) -> [f64; 3] {
        structure
            .lattice
            .to_cartesian(transform.origin().map(Rational::to_f64))
    }

    #[test]
    fn origin_choices_unique_axes_and_hexagonal_axes() {
        // Silicon in F d -3 m, origin choice 2, to origin choice 1: the
        // origin moves to −1/8, −1/8, −1/8 … and the atom to 0, 0, 0.
        let silicon = structure(
            "F d -3 m :2",
            cubic(5.431),
            vec![Site::single(1, "Si1", "Si", [0.125, 0.125, 0.125])],
        );
        let one = tables::find_hm("F d -3 m :1").unwrap().hall_number;
        let choice = CellPreset::Setting { hall_number: one }
            .transform(&silicon)
            .unwrap();
        assert_eq!(choice.matrix(), CellTransform::IDENTITY.matrix());
        let moved = apply(&silicon, choice).unwrap();
        assert_eq!(
            tables::match_operations(&moved.symmetry).map(|s| s.code),
            Some("1")
        );
        assert_eq!(Derived::compute(&moved).atoms_in_cell(), 8);
        assert!(
            moved.sites[0]
                .position
                .iter()
                .all(|v| [0.0, 0.25, -0.25].contains(v))
        );

        // P 1 21/c 1 to P 1 1 21/a (unique axis c).
        let p21c = structure(
            "P 1 21/c 1",
            CellParams::new(5.0, 6.0, 7.0, 90.0, 100.0, 90.0),
            vec![Site::single(1, "C1", "C", [0.1, 0.2, 0.3])],
        );
        let c1 = tables::find_hm("P 1 1 21/a").unwrap().hall_number;
        let choices: Vec<u16> = other_settings(&p21c)
            .iter()
            .map(|s| s.hall_number)
            .collect();
        assert_eq!(choices.len(), 8);
        assert!(choices.contains(&c1));
        let turned = apply(
            &p21c,
            CellPreset::Setting { hall_number: c1 }
                .transform(&p21c)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(turned.lattice.params().gamma.value(), 100.0);
        assert!(same_crystal(&turned, &p21c, [0.0; 3]));

        // Corundum, R -3 c in hexagonal axes, to rhombohedral axes: a third
        // of the cell.
        let corundum = structure(
            "R -3 c",
            CellParams::new(4.759, 4.759, 12.991, 90.0, 90.0, 120.0),
            vec![
                Site::single(1, "Al1", "Al", [0.0, 0.0, 0.35216]),
                Site::single(2, "O1", "O", [0.30624, 0.0, 0.25]),
            ],
        );
        let rhombohedral = tables::find_hm("R -3 c :R").unwrap().hall_number;
        let transform = CellPreset::Setting {
            hall_number: rhombohedral,
        }
        .transform(&corundum)
        .unwrap();
        assert_eq!(transform.determinant(), r("1/3"));
        let small = apply(&corundum, transform).unwrap();
        assert_eq!(Derived::compute(&small).atoms_in_cell(), 10);
        let p = small.lattice.params();
        assert!((p.a.value() - p.b.value()).abs() < 1e-12);
        assert!((p.alpha.value() - 55.28).abs() < 0.01, "{p:?}");
        let back = apply(&small, CellPreset::Conventional.transform(&small).unwrap()).unwrap();
        assert_eq!(back, corundum);
    }

    #[test]
    fn presets_that_change_nothing_are_the_identity() {
        let rutile = rutile();
        for preset in [CellPreset::Primitive, CellPreset::Conventional] {
            assert_eq!(preset.transform(&rutile), Some(CellTransform::IDENTITY));
        }
        // a < c is not reduced: Niggli puts c first.
        let niggli = CellPreset::Niggli.transform(&rutile).unwrap();
        assert!(!niggli.is_identity());
        let reduced = apply(&rutile, niggli).unwrap();
        let p = reduced.lattice.params();
        assert_eq!(
            [p.a.value(), p.b.value(), p.c.value()],
            [2.959, 4.594, 4.594]
        );
        assert_eq!(
            CellPreset::Niggli.transform(&reduced),
            Some(CellTransform::IDENTITY)
        );
        // The structure's own setting and other types are not offered.
        let own = tables::match_operations(&rutile.symmetry)
            .unwrap()
            .hall_number;
        assert_eq!(
            CellPreset::Setting { hall_number: own }.transform(&rutile),
            Some(CellTransform::IDENTITY)
        );
        assert_eq!(
            CellPreset::Setting { hall_number: 1 }.transform(&rutile),
            None
        );
        assert_eq!(
            CellPreset::Setting { hall_number: 999 }.transform(&rutile),
            None
        );
        assert!(other_settings(&rutile).is_empty());
        // An identity change adds no undo step.
        let mut document = CrystalDocument::new(rutile.clone());
        let outcome = document
            .edit(Edit::TransformCell {
                transform: CellTransform::IDENTITY,
            })
            .unwrap();
        assert!(!outcome.changed);
    }

    #[test]
    fn explicit_and_left_handed_orientations_keep_the_atoms_in_place() {
        let nacl = structure(
            "F m -3 m",
            cubic(5.64),
            vec![
                Site::single(1, "Na1", "Na", [0.0, 0.0, 0.0]),
                Site::single(2, "Cl1", "Cl", [0.5, 0.5, 0.5]),
            ],
        );
        let (s, c) = 0.4f64.sin_cos();
        let turned = Rotation::new([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]).unwrap();
        let mirrored = Rotation::new([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, -1.0]]).unwrap();
        for rotation in [turned, mirrored] {
            let mut oriented = nacl.clone();
            oriented.lattice =
                Lattice::with_orientation(*nacl.lattice.params(), Orientation::new(rotation));
            let left = oriented.lattice.is_left_handed();
            for preset in [CellPreset::Primitive, CellPreset::Niggli] {
                let transform = preset.transform(&oriented).unwrap();
                let small = apply(&oriented, transform).unwrap();
                assert_eq!(small.lattice.is_left_handed(), left, "{preset:?}");
                assert!(same_crystal(&small, &oriented, [0.0; 3]), "{preset:?}");
                // Back to the conventional cell, in place up to rounding.
                let back =
                    apply(&small, CellPreset::Conventional.transform(&small).unwrap()).unwrap();
                assert!(same_crystal(&back, &oriented, [0.0; 3]));
                assert_eq!(back.lattice.is_left_handed(), left);
                assert!(matches!(
                    back.lattice.orientation(),
                    Orientation::Explicit(_)
                ));
            }
        }
        // A diagonal matrix keeps the orientation as it is.
        let mut oriented = rutile();
        oriented.lattice =
            Lattice::with_orientation(*oriented.lattice.params(), Orientation::new(mirrored));
        let double = transform(
            [["1", "0", "0"], ["0", "1", "0"], ["0", "0", "2"]],
            ["0"; 3],
        );
        let tall = apply(&oriented, double).unwrap();
        assert_eq!(tall.lattice.orientation(), oriented.lattice.orientation());
        assert!(same_crystal(&tall, &oriented, [0.0; 3]));
        // One that turns two axes around turns the cell, not the atoms.
        let around = transform(
            [["-1", "0", "0"], ["0", "-1", "0"], ["0", "0", "1"]],
            ["0"; 3],
        );
        for structure in [rutile(), oriented] {
            let turned = apply(&structure, around).unwrap();
            assert_ne!(
                turned.lattice.orientation(),
                structure.lattice.orientation()
            );
            assert_eq!(turned.lattice.params(), structure.lattice.params());
            assert!(same_crystal(&turned, &structure, [0.0; 3]));
        }
    }

    #[test]
    fn displacement_parameters_and_uncertainties_follow_the_basis() {
        let u = [
            Measured::with_su(0.0110, 0.0002),
            Measured::with_su(0.0120, 0.0003),
            Measured::with_su(0.0130, 0.0004),
            Measured::with_su(0.0010, 0.0001),
            Measured::exact(0.0020),
            Measured::with_su(0.0030, 0.0001),
        ];
        let mut site = Site::single(1, "Fe1", "Fe", [0.0713, 0.3291, 0.1847]).with_position_su([
            Some(0.0002),
            Some(0.0003),
            Some(0.0004),
        ]);
        site.occupants[0].adp = Some(Adp::Anisotropic(u));
        let params = CellParams {
            a: Measured::with_su(5.1, 0.001),
            b: Measured::with_su(6.2, 0.002),
            c: Measured::with_su(7.3, 0.003),
            alpha: Measured::exact(90.0),
            beta: Measured::with_su(98.0, 0.01),
            gamma: Measured::exact(90.0),
        };
        let original = structure("C 1 2/c 1", params, vec![site]);

        // Axes permuted: a′ = c, b′ = −b, c′ = a. Everything carries over
        // exactly, uncertainties included.
        let permuted = transform(
            [["0", "0", "1"], ["0", "-1", "0"], ["1", "0", "0"]],
            ["0"; 3],
        );
        let turned = apply(&original, permuted).unwrap();
        let p = turned.lattice.params();
        assert_eq!((p.a, p.b, p.c), (params.c, params.b, params.a));
        assert_eq!(p.beta, params.beta);
        assert_eq!(turned.sites[0].position, [0.1847, -0.3291, 0.0713]);
        assert_eq!(
            turned.sites[0].position_su,
            [Some(0.0004), Some(0.0003), Some(0.0002)]
        );
        let Some(Adp::Anisotropic(v)) = turned.sites[0].occupants[0].adp else {
            panic!("anisotropic")
        };
        // U11′ = U33, U22′ = U22, U33′ = U11, U12′ = −U23, U13′ = U13, U23′ = −U12.
        assert_eq!(v[0], u[2]);
        assert_eq!(v[1], u[1]);
        assert_eq!(v[2], u[0]);
        assert_eq!(v[3], Measured::with_su(-0.0030, 0.0001));
        assert_eq!(v[4], u[4]);
        assert_eq!(v[5], Measured::with_su(-0.0010, 0.0001));

        // To the primitive cell and back: the values come back exactly; the
        // cell's uncertainties are lost (their correlations are not known),
        // those of coordinates and displacement parameters add in quadrature.
        let primitive = CellPreset::Primitive.transform(&original).unwrap();
        let small = apply(&original, primitive).unwrap();
        assert_eq!(small.lattice.params().a.su(), None);
        assert_eq!(small.lattice.params().c, params.c);
        let su = small.sites[0].position_su;
        assert!((su[0].unwrap() - (0.0002f64.powi(2) + 0.0003f64.powi(2)).sqrt()).abs() < 1e-15);
        assert_eq!(su[2], Some(0.0004));
        // The thermal ellipsoid stays the same in Cartesian space.
        assert_close(&cartesian_u(&small), &cartesian_u(&original));
        let back = apply(&small, CellPreset::Conventional.transform(&small).unwrap()).unwrap();
        assert_eq!(back.sites[0].position, original.sites[0].position);
        let Some(Adp::Anisotropic(w)) = back.sites[0].occupants[0].adp else {
            panic!("anisotropic")
        };
        assert_eq!(w.map(|m| m.value()), u.map(|m| m.value()));
        assert_close(&cartesian_u(&back), &cartesian_u(&original));
    }

    /// The first site's displacement tensor in Cartesian Å²: `A·N·U·N·Aᵀ`.
    fn cartesian_u(structure: &Structure) -> [[f64; 3]; 3] {
        let Some(Adp::Anisotropic(u)) = structure.sites[0].occupants[0].adp else {
            panic!("anisotropic")
        };
        let u = u.map(|m| m.value());
        let full = [[u[0], u[3], u[4]], [u[3], u[1], u[5]], [u[4], u[5], u[2]]];
        let n = structure.lattice.params().reciprocal_lengths();
        let a = nalgebra::Matrix3::from(structure.lattice.matrix());
        let b = nalgebra::Matrix3::from_fn(|i, j| n[i] * full[i][j] * n[j]);
        let c = a * b * a.transpose();
        [0, 1, 2].map(|i| [0, 1, 2].map(|j| c[(i, j)]))
    }

    fn assert_close(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) {
        for (x, y) in a.iter().flatten().zip(b.iter().flatten()) {
            assert!((x - y).abs() < 1e-12, "{a:?} {b:?}");
        }
    }

    #[test]
    fn references_follow_the_atoms() {
        let nacl = structure(
            "F m -3 m",
            cubic(5.64),
            vec![
                Site::single(1, "Na1", "Na", [0.0, 0.0, 0.0]),
                Site::single(2, "Cl1", "Cl", [0.5, 0.5, 0.5]),
            ],
        );
        let before = Derived::compute(&nacl);
        let shift = transform(
            [
                ["0", "1/2", "1/2"],
                ["1/2", "0", "1/2"],
                ["1/2", "1/2", "0"],
            ],
            ["1/4", "0", "0"],
        );
        let applied = Edit::TransformCell { transform: shift }
            .apply(&nacl)
            .unwrap();
        let after = Derived::compute(&applied.structure);
        let offset = shift_of(&nacl, &shift);
        // Every atom drawn, each face and corner image included, is the
        // same atom after the change, wherever it now lies.
        for atom in &before.atoms {
            let AtomFate::Kept(kept) = applied.ids.atom(atom.atom) else {
                panic!("{:?} lost", atom.atom)
            };
            let x = after.fractional(kept).expect("an atom of the new cell");
            let r = applied.structure.lattice.to_cartesian(x);
            let expected = nacl
                .lattice
                .to_cartesian(before.fractional(atom.atom).unwrap());
            for i in 0..3 {
                assert!((r[i] - (expected[i] - offset[i])).abs() < 1e-9, "{atom:?}");
            }
        }
        assert_eq!(applied.ids.sites(SiteId(2)), [SiteId(2)]);

        // In a document, measurements keep their atoms through the change,
        // undo and redo, which are exact.
        let mut document = CrystalDocument::new(nacl.clone());
        let na = AtomRef::new(SiteId(1), 0, [0; 3]);
        let cl = AtomRef::new(SiteId(2), 3, [0, 1, 0]);
        let distance = Measurement::Distance([na, cl]);
        document.execute(Command::AddMeasurement(distance)).unwrap();
        let length = |document: &CrystalDocument| {
            document.annotations()[0]
                .measurement
                .value(&document.structure().lattice, document.derived())
                .unwrap()
        };
        let before = length(&document);
        document
            .edit(Edit::TransformCell { transform: shift })
            .unwrap();
        assert!(!document.annotations()[0].stale);
        assert!((length(&document) - before).abs() < 1e-9);
        let transformed = document.snapshot();
        document.undo().unwrap();
        assert_eq!(*document.structure(), nacl);
        assert_eq!(document.annotations()[0].measurement, distance);
        document.redo().unwrap();
        assert_eq!(*document.structure(), *transformed);
        assert!((length(&document) - before).abs() < 1e-9);
    }

    #[test]
    fn transformations_serialize_as_fractions() {
        let shift = transform(
            [["1", "-1", "0"], ["1", "1", "0"], ["0", "0", "1"]],
            ["0", "0", "1/4"],
        );
        let edit = Edit::TransformCell { transform: shift };
        let json = serde_json::to_string(&edit).unwrap();
        assert_eq!(
            json,
            r#"{"command":"transform_cell","transform":{"matrix":[["1","-1","0"],["1","1","0"],["0","0","1"]],"origin":["0","0","1/4"]}}"#
        );
        assert_eq!(serde_json::from_str::<Edit>(&json).unwrap(), edit);
        assert_eq!(shift.to_string(), "a+b, -a+b, c; 0, 0, 1/4");
        // Without an origin, and refused when the matrix is singular.
        let read: CellTransform =
            serde_json::from_str(r#"{"matrix":[["2","0","0"],["0","1","0"],["0","0","1"]]}"#)
                .unwrap();
        assert_eq!(read.determinant(), r("2"));
        assert!(
            serde_json::from_str::<CellTransform>(
                r#"{"matrix":[["1","1","0"],["1","1","0"],["0","0","1"]]}"#
            )
            .is_err()
        );
        // The inverse of the inverse is the change itself.
        assert_eq!(shift.inverse().unwrap().inverse().unwrap(), shift);
        assert_eq!(
            shift.inverse().unwrap(),
            transform(
                [["1/2", "1/2", "0"], ["-1/2", "1/2", "0"], ["0", "0", "1"]],
                ["0", "0", "-1/4"]
            )
        );
    }

    #[test]
    fn fractions_parse() {
        assert_eq!(Rational::parse(" -1/2 "), Some(Rational::new(-1, 2)));
        assert_eq!(Rational::parse("3"), Some(Rational::integer(3)));
        assert_eq!(Rational::parse("0.5"), Some(Rational::new(1, 2)));
        assert_eq!(Rational::parse("0.3333"), Some(Rational::new(1, 3)));
        assert_eq!(Rational::parse("0.3"), None);
        assert_eq!(Rational::parse("1/0"), None);
        assert_eq!(Rational::parse("inf"), None);
        assert_eq!(Rational::parse(""), None);
    }

    #[test]
    fn p1_structures() {
        let p1 = crate::transform::expand_to_p1(&rutile()).structure;
        assert_eq!(
            CellPreset::Primitive.transform(&p1),
            Some(CellTransform::IDENTITY)
        );
        assert!(other_settings(&p1).is_empty());
        // P1 has one setting, which the structure is in; its reduced cell
        // is the Niggli preset.
        assert_eq!(
            CellPreset::Conventional.transform(&p1),
            Some(CellTransform::IDENTITY)
        );
        let niggli = CellPreset::Niggli.transform(&p1).unwrap();
        let reduced = apply(&p1, niggli).unwrap();
        assert!(reduced.symmetry.is_p1());
        let p = reduced.lattice.params();
        assert_eq!(
            [p.a.value(), p.b.value(), p.c.value()],
            [2.959, 4.594, 4.594]
        );
        // Halving the cell would make two atoms one.
        let half = transform(
            [["1/2", "0", "0"], ["0", "1", "0"], ["0", "0", "1"]],
            ["0"; 3],
        );
        assert_eq!(
            apply(&p1, half),
            Err(EditError::Transform(TransformError::NotATranslation {
                vector: 0
            }))
        );
    }
}
