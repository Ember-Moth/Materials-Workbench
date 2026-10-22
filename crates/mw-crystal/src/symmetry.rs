//! Symmetry operations and operation sets (design §4.4).
//!
//! An operation is `x' = W·x + w` with an integer matrix `W` and a rational
//! translation `w` taken modulo 1, so operations that differ by a lattice
//! translation are the same operation. The operation set is the source of
//! truth for a structure's symmetry; space group names only label it.

use std::collections::HashSet;
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};

/// An exact rational number in lowest terms with a positive denominator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    num: i64,
    den: i64,
}

/// How far a decimal may be from a fraction and still be read as it, e.g.
/// `0.333` as 1/3. Fractions with denominators dividing 24 are 1/24 apart,
/// so this cannot pick the wrong one.
const DECIMAL_TOLERANCE: f64 = 2e-3;

impl Rational {
    pub const ZERO: Rational = Rational { num: 0, den: 1 };
    pub const ONE: Rational = Rational { num: 1, den: 1 };

    /// `num / den`. Panics if `den` is zero.
    pub fn new(num: i64, den: i64) -> Self {
        assert!(den != 0, "zero denominator");
        let divisor = gcd(num, den).max(1) * den.signum();
        Self {
            num: num / divisor,
            den: den / divisor,
        }
    }

    pub fn integer(value: i64) -> Self {
        Self { num: value, den: 1 }
    }

    /// Reads a decimal as the fraction it abbreviates, e.g. `0.3333` as 1/3.
    /// Only fractions whose denominator divides 24 are candidates, which
    /// covers every translation of the space group tables.
    pub fn from_decimal(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let scaled = value * 24.0;
        let nearest = scaled.round();
        ((scaled - nearest).abs() <= DECIMAL_TOLERANCE * 24.0)
            .then(|| Self::new(nearest as i64, 24))
    }

    pub fn numerator(self) -> i64 {
        self.num
    }

    pub fn denominator(self) -> i64 {
        self.den
    }

    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// The same value modulo 1, in [0, 1).
    pub fn fract(self) -> Self {
        Self::new(self.num.rem_euclid(self.den), self.den)
    }

    pub fn is_zero(self) -> bool {
        self.num == 0
    }
}

fn gcd(a: i64, b: i64) -> i64 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Add for Rational {
    type Output = Rational;

    fn add(self, other: Rational) -> Rational {
        Rational::new(
            self.num * other.den + other.num * self.den,
            self.den * other.den,
        )
    }
}

impl Sub for Rational {
    type Output = Rational;

    fn sub(self, other: Rational) -> Rational {
        self + -other
    }
}

impl Mul for Rational {
    type Output = Rational;

    fn mul(self, other: Rational) -> Rational {
        Rational::new(self.num * other.num, self.den * other.den)
    }
}

impl Div for Rational {
    type Output = Rational;

    /// Panics if `other` is zero.
    fn div(self, other: Rational) -> Rational {
        Rational::new(self.num * other.den, self.den * other.num)
    }
}

impl Mul<i64> for Rational {
    type Output = Rational;

    fn mul(self, factor: i64) -> Rational {
        Rational::new(self.num * factor, self.den)
    }
}

impl Neg for Rational {
    type Output = Rational;

    fn neg(self) -> Rational {
        Rational::new(-self.num, self.den)
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

/// Why a symmetry operation string such as `x+1/2, -y, z` was rejected.
#[derive(Clone, Debug, PartialEq)]
pub enum SymOpError {
    /// Not three comma-separated components.
    ComponentCount(usize),
    /// A character or term outside the accepted form, e.g. `x*y`.
    Syntax,
    /// A coefficient of x, y or z that is not an integer, e.g. `0.5x`.
    NonIntegerCoefficient,
    /// A decimal translation that is not close to a fraction with a
    /// denominator dividing 24.
    UnrecognizedDecimal(f64),
}

impl fmt::Display for SymOpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ComponentCount(n) => write!(f, "expected 3 components, found {n}"),
            Self::Syntax => write!(f, "not of the form x+1/2, -y, z"),
            Self::NonIntegerCoefficient => write!(f, "coefficient of x, y or z is not an integer"),
            Self::UnrecognizedDecimal(v) => write!(f, "{v} is not close to a simple fraction"),
        }
    }
}

impl std::error::Error for SymOpError {}

/// One symmetry operation `x' = W·x + w`; `rotation[i][j]` is the
/// coefficient of coordinate `j` in component `i`. The translation is kept
/// modulo 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SymOp {
    rotation: [[i32; 3]; 3],
    translation: [Rational; 3],
}

impl SymOp {
    pub const IDENTITY: SymOp = SymOp {
        rotation: [[1, 0, 0], [0, 1, 0], [0, 0, 1]],
        translation: [Rational::ZERO; 3],
    };

    pub fn new(rotation: [[i32; 3]; 3], translation: [Rational; 3]) -> Self {
        Self {
            rotation,
            translation: translation.map(Rational::fract),
        }
    }

    /// Parses the CIF notation, e.g. `x+1/2, -y, z` or `1/2-X,Y,0.3333+Z`.
    /// Decimal translations are read as fractions within a tolerance and
    /// rejected when they match none.
    pub fn parse(text: &str) -> Result<Self, SymOpError> {
        let components: Vec<&str> = text.split(',').collect();
        if components.len() != 3 {
            return Err(SymOpError::ComponentCount(components.len()));
        }
        let mut rotation = [[0; 3]; 3];
        let mut translation = [Rational::ZERO; 3];
        for (i, component) in components.iter().enumerate() {
            (rotation[i], translation[i]) = parse_component(component)?;
        }
        Ok(Self::new(rotation, translation))
    }

    pub fn rotation(&self) -> &[[i32; 3]; 3] {
        &self.rotation
    }

    pub fn translation(&self) -> &[Rational; 3] {
        &self.translation
    }

    pub fn determinant(&self) -> i32 {
        let m = &self.rotation;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }

    /// Applies the operation to fractional coordinates.
    pub fn apply(&self, x: [f64; 3]) -> [f64; 3] {
        [0, 1, 2].map(|i| {
            (0..3)
                .map(|j| f64::from(self.rotation[i][j]) * x[j])
                .sum::<f64>()
                + self.translation[i].to_f64()
        })
    }

    /// The operation that applies `first` and then `self`, modulo lattice
    /// translations.
    pub fn after(&self, first: &SymOp) -> SymOp {
        // W = W₂·W₁, w = W₂·w₁ + w₂.
        let rotation = [0, 1, 2].map(|i| {
            [0, 1, 2].map(|j| {
                (0..3)
                    .map(|k| self.rotation[i][k] * first.rotation[k][j])
                    .sum()
            })
        });
        let translation = [0, 1, 2].map(|i| {
            (0..3).fold(self.translation[i], |sum, j| {
                sum + first.translation[j] * i64::from(self.rotation[i][j])
            })
        });
        SymOp::new(rotation, translation)
    }
}

/// Parses one component such as `-x+y+1/3` into its row of `W` and its
/// translation.
fn parse_component(text: &str) -> Result<([i32; 3], Rational), SymOpError> {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.is_empty() {
        return Err(SymOpError::Syntax);
    }
    let mut row = [0; 3];
    let mut translation = Rational::ZERO;
    let mut i = 0;
    while i < chars.len() {
        let sign = match chars[i] {
            '+' => {
                i += 1;
                1
            }
            '-' => {
                i += 1;
                -1
            }
            // Only the first term may omit its sign.
            _ if i == 0 => 1,
            _ => return Err(SymOpError::Syntax),
        };
        let number = parse_number(&chars, &mut i)?;
        if chars.get(i) == Some(&'*') {
            i += 1;
        }
        match chars.get(i).map(|c| c.to_ascii_lowercase()) {
            Some(axis @ ('x' | 'y' | 'z')) => {
                i += 1;
                let coefficient = match number {
                    None => 1,
                    Some(Number::Exact(r)) if r.denominator() == 1 => r.numerator(),
                    Some(Number::Decimal(v)) if v.fract() == 0.0 => v as i64,
                    Some(_) => return Err(SymOpError::NonIntegerCoefficient),
                };
                let axis = usize::from(axis as u8 - b'x');
                row[axis] += sign * i32::try_from(coefficient).map_err(|_| SymOpError::Syntax)?;
            }
            Some('+' | '-') | None => {
                let value = match number {
                    Some(Number::Exact(r)) => r,
                    Some(Number::Decimal(v)) => {
                        Rational::from_decimal(v).ok_or(SymOpError::UnrecognizedDecimal(v))?
                    }
                    None => return Err(SymOpError::Syntax),
                };
                translation = translation + value * i64::from(sign);
            }
            Some(_) => return Err(SymOpError::Syntax),
        }
        // A term ends at the next sign or at the end of the component.
        if i < chars.len() && !matches!(chars[i], '+' | '-') {
            return Err(SymOpError::Syntax);
        }
    }
    Ok((row, translation))
}

enum Number {
    /// An integer or a fraction such as `1/2`.
    Exact(Rational),
    Decimal(f64),
}

fn parse_number(chars: &[char], i: &mut usize) -> Result<Option<Number>, SymOpError> {
    let digits = |i: &mut usize| {
        let start = *i;
        while chars.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
        chars[start..*i].iter().collect::<String>()
    };
    let whole = digits(i);
    if chars.get(*i) == Some(&'.') {
        *i += 1;
        let fraction = digits(i);
        if whole.is_empty() && fraction.is_empty() {
            return Err(SymOpError::Syntax);
        }
        let value: f64 = format!("{whole}.{fraction}0")
            .parse()
            .map_err(|_| SymOpError::Syntax)?;
        return Ok(Some(Number::Decimal(value)));
    }
    if whole.is_empty() {
        return Ok(None);
    }
    let numerator: i64 = whole.parse().map_err(|_| SymOpError::Syntax)?;
    if chars.get(*i) == Some(&'/') {
        *i += 1;
        let denominator: i64 = digits(i).parse().map_err(|_| SymOpError::Syntax)?;
        if denominator == 0 {
            return Err(SymOpError::Syntax);
        }
        return Ok(Some(Number::Exact(Rational::new(numerator, denominator))));
    }
    Ok(Some(Number::Exact(Rational::integer(numerator))))
}

impl fmt::Display for SymOp {
    /// Writes the CIF notation, e.g. `-y+1/2,x+1/2,z+1/2`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (row, translation)) in self.rotation.iter().zip(self.translation).enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            let mut text = String::new();
            for (coefficient, axis) in row.iter().zip(["x", "y", "z"]) {
                match coefficient {
                    0 => {}
                    1 => text.push_str(&format!("+{axis}")),
                    -1 => text.push_str(&format!("-{axis}")),
                    c => text.push_str(&format!("{c:+}{axis}")),
                }
            }
            if !translation.is_zero() {
                text.push_str(&format!("+{translation}"));
            }
            let text = text.strip_prefix('+').unwrap_or(&text);
            f.write_str(if text.is_empty() { "0" } else { text })?;
        }
        Ok(())
    }
}

/// Why a list of operations is not a valid operation set.
#[derive(Clone, Debug, PartialEq)]
pub enum SymmetryError {
    MissingIdentity,
    /// The operation at this index has a rotation with determinant ≠ ±1.
    Singular(usize),
    /// Two entries are the same operation modulo lattice translations.
    Duplicate(usize, usize),
    /// The product of two operations (indices into the list) is missing,
    /// so the set is not a group.
    NotClosed(usize, usize),
}

impl fmt::Display for SymmetryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingIdentity => write!(f, "the identity x,y,z is missing"),
            Self::Singular(i) => write!(f, "operation {} is not invertible", i + 1),
            Self::Duplicate(a, b) => write!(f, "operations {} and {} are the same", a + 1, b + 1),
            Self::NotClosed(a, b) => write!(
                f,
                "the product of operations {} and {} is missing",
                a + 1,
                b + 1
            ),
        }
    }
}

impl std::error::Error for SymmetryError {}

/// The largest operation set of a space group (Fm-3m in its conventional
/// cell); closing generators beyond it means they generate no space group.
const MAX_OPERATIONS: usize = 192;

/// A validated set of symmetry operations: it contains the identity, every
/// rotation is invertible over the integers, and it is closed under
/// composition modulo lattice translations. The identity comes first, so the
/// first member of every orbit is the site itself (design §5.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Symmetry {
    operations: Vec<SymOp>,
}

impl Symmetry {
    /// Only the identity.
    pub fn p1() -> Self {
        Self {
            operations: vec![SymOp::IDENTITY],
        }
    }

    pub fn new(mut operations: Vec<SymOp>) -> Result<Self, SymmetryError> {
        let identity = operations
            .iter()
            .position(SymOp::is_identity)
            .ok_or(SymmetryError::MissingIdentity)?;
        if let Some(i) = operations.iter().position(|op| op.determinant().abs() != 1) {
            return Err(SymmetryError::Singular(i));
        }
        let mut seen = std::collections::HashMap::new();
        for (i, op) in operations.iter().enumerate() {
            if let Some(first) = seen.insert(*op, i) {
                return Err(SymmetryError::Duplicate(first, i));
            }
        }
        for (a, op_a) in operations.iter().enumerate() {
            for (b, op_b) in operations.iter().enumerate() {
                if !seen.contains_key(&op_a.after(op_b)) {
                    return Err(SymmetryError::NotClosed(a, b));
                }
            }
        }
        let identity = operations.remove(identity);
        operations.insert(0, identity);
        Ok(Self { operations })
    }

    /// An operation set known to be a group, such as a tabulated setting.
    /// Moves the identity to the front.
    pub(crate) fn from_group(mut operations: Vec<SymOp>) -> Self {
        if let Some(i) = operations.iter().position(SymOp::is_identity) {
            let identity = operations.remove(i);
            operations.insert(0, identity);
        }
        Self { operations }
    }

    /// The group generated by `generators`, or `None` if they generate more
    /// operations than any space group has.
    pub fn generate(generators: &[SymOp]) -> Option<Self> {
        let mut operations = vec![SymOp::IDENTITY];
        let mut seen: HashSet<SymOp> = operations.iter().copied().collect();
        let mut i = 0;
        while i < operations.len() {
            for generator in generators {
                let product = generator.after(&operations[i]);
                if seen.insert(product) {
                    operations.push(product);
                    if operations.len() > MAX_OPERATIONS {
                        return None;
                    }
                }
            }
            i += 1;
        }
        Some(Self { operations })
    }

    pub fn operations(&self) -> &[SymOp] {
        &self.operations
    }

    /// Whether the set holds only the identity.
    pub fn is_p1(&self) -> bool {
        self.operations.len() == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(text: &str) -> SymOp {
        SymOp::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn parses_the_usual_forms() {
        let parsed = op("x+1/2, -y, z");
        assert_eq!(parsed.rotation(), &[[1, 0, 0], [0, -1, 0], [0, 0, 1]]);
        assert_eq!(parsed.translation()[0], Rational::new(1, 2));
        assert_eq!(op("1/2+X,-Y,Z"), parsed);
        assert_eq!(op("+x+0.5,-y,+z"), parsed);
        assert_eq!(op("x-1/2,-y+1,z"), parsed);
        assert_eq!(op("-x+y, -x, z+2/3").to_string(), "-x+y,-x,z+2/3");
    }

    #[test]
    fn reads_decimals_as_fractions() {
        assert_eq!(
            op("x+0.3333,y+0.6667,z+0.1667").to_string(),
            "x+1/3,y+2/3,z+1/6"
        );
        assert_eq!(op("x+.25,y-0.125,z").to_string(), "x+1/4,y+7/8,z");
        assert_eq!(
            SymOp::parse("x+0.3,y,z"),
            Err(SymOpError::UnrecognizedDecimal(0.3))
        );
    }

    #[test]
    fn rejects_malformed_operations() {
        assert_eq!(SymOp::parse("x,y"), Err(SymOpError::ComponentCount(2)));
        assert_eq!(SymOp::parse("x,y,"), Err(SymOpError::Syntax));
        assert_eq!(SymOp::parse("x,y,zz"), Err(SymOpError::Syntax));
        assert_eq!(SymOp::parse("x,y,a"), Err(SymOpError::Syntax));
        assert_eq!(SymOp::parse("x y,y,z"), Err(SymOpError::Syntax));
        assert_eq!(
            SymOp::parse("0.5x,y,z"),
            Err(SymOpError::NonIntegerCoefficient)
        );
    }

    #[test]
    fn display_round_trips() {
        for text in ["x,y,z", "-y+1/2,x+1/2,z+1/2", "x-y,x,-z+5/6", "-x,-y,-z"] {
            assert_eq!(op(text).to_string(), text);
            assert_eq!(op(&op(text).to_string()), op(text));
        }
    }

    #[test]
    fn composition_is_modulo_lattice_translations() {
        let screw = op("-x,y+1/2,-z");
        assert!(screw.after(&screw).is_identity());
        let four = op("-y,x,z+1/4");
        let mut power = SymOp::IDENTITY;
        for _ in 0..4 {
            power = four.after(&power);
        }
        assert!(power.is_identity());
    }

    #[test]
    fn apply_matches_the_matrix() {
        let image = op("-y+1/2,x,z+1/4").apply([0.1, 0.2, 0.3]);
        for (a, b) in image.iter().zip([0.3, 0.1, 0.55]) {
            assert!((a - b).abs() < 1e-12, "{image:?}");
        }
    }

    #[test]
    fn generated_groups_have_the_tabulated_orders() {
        // P4₂/mnm (136), Pm-3m (221), Fm-3m (225), P6₃/mmc (194).
        let cases: [(&[&str], usize); 4] = [
            (
                &["-y+1/2,x+1/2,z+1/2", "-x+1/2,y+1/2,-z+1/2", "-x,-y,-z"],
                16,
            ),
            (&["-y,x,z", "z,x,y", "-x,-y,-z"], 48),
            (
                &[
                    "-y,x,z",
                    "z,x,y",
                    "-x,-y,-z",
                    "x,y+1/2,z+1/2",
                    "x+1/2,y,z+1/2",
                ],
                192,
            ),
            (&["x-y,x,z+1/2", "y,x,-z", "-x,-y,-z"], 24),
        ];
        for (generators, order) in cases {
            let generators: Vec<SymOp> = generators.iter().map(|g| op(g)).collect();
            let group = Symmetry::generate(&generators).expect("finite");
            assert_eq!(group.operations().len(), order, "{generators:?}");
            assert_eq!(Symmetry::new(group.operations().to_vec()), Ok(group));
        }
    }

    #[test]
    fn rejects_sets_that_are_not_groups() {
        let ops = |texts: &[&str]| texts.iter().map(|t| op(t)).collect::<Vec<_>>();
        assert_eq!(
            Symmetry::new(ops(&["-x,-y,-z"])),
            Err(SymmetryError::MissingIdentity)
        );
        assert_eq!(
            Symmetry::new(ops(&["x,y,z", "-y,x,z"])),
            Err(SymmetryError::NotClosed(1, 1))
        );
        assert_eq!(
            Symmetry::new(ops(&["x,y,z", "-x,-y,-z", "-x,-y,-z+1"])),
            Err(SymmetryError::Duplicate(1, 2))
        );
        assert_eq!(
            Symmetry::new(ops(&["x,y,z", "x,x,z"])),
            Err(SymmetryError::Singular(1))
        );
    }

    #[test]
    fn identity_moves_to_the_front() {
        let symmetry = Symmetry::new(vec![op("-x,-y,-z"), SymOp::IDENTITY]).unwrap();
        assert!(symmetry.operations()[0].is_identity());
        assert!(!symmetry.is_p1());
        assert!(Symmetry::p1().is_p1());
    }

    #[test]
    fn unlimited_generators_are_refused() {
        assert!(Symmetry::generate(&[op("x+y,y,z")]).is_none());
    }
}
