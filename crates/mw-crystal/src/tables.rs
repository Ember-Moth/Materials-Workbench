//! The space group tables and the symmetry search, from moyo (design §11):
//! the 530 tabulated settings with their symbols, operations and Wyckoff
//! positions. Every use of moyo is in this module, so that changes to its
//! 0.x API stay here.

use std::collections::HashMap;
use std::sync::LazyLock;

use moyo::MoyoDataset;
use moyo::base::{AngleTolerance, Cell, Lattice, Operation};
use moyo::data::{
    HallSymbol, Setting as MoyoSetting, hall_symbol_entry, iter_wyckoff_positions_from_hall_number,
};
use nalgebra::{Matrix3, Vector3};

use crate::{Rational, SymOp, Symmetry};

/// One tabulated setting of a space group type, identified by its Hall
/// number (1–530).
pub(crate) struct Setting {
    pub hall_number: u16,
    /// International Tables number, 1–230.
    pub number: u16,
    /// Short symbol of the type, the same for all its settings, e.g.
    /// `P 2_1/c`.
    pub hm_short: &'static str,
    /// Full symbol of this setting, e.g. `P 1 1 2_1/a`.
    pub hm_full: &'static str,
    /// Unique axis and cell choice (`b1`, `c2`), axes (`ba-c`), origin
    /// choice (`1`, `2`) or hexagonal / rhombohedral axes (`H`, `R`); empty
    /// for types with one setting.
    pub code: &'static str,
    /// The operations of the conventional cell, centering included.
    pub symmetry: Symmetry,
}

impl Setting {
    /// Whether this is the setting the International Tables list first:
    /// unique axis b, cell choice 1, origin choice 2, hexagonal axes.
    pub fn is_standard(&self) -> bool {
        standard(self.number).hall_number == self.hall_number
    }
}

/// One Wyckoff position of a tabulated setting.
pub(crate) struct WyckoffEntry {
    /// In the conventional cell.
    pub multiplicity: usize,
    pub letter: char,
    /// Oriented site symmetry symbol, e.g. `m.2m`.
    pub site_symmetry: &'static str,
    /// The coordinate triplet, e.g. `x,x,0`, as the affine map from the
    /// free parameters (x, y, z) to the position.
    pub coordinates: SymOp,
}

struct Tables {
    settings: Vec<Setting>,
    /// Hall number by sorted operation set. Three pairs of settings of
    /// No. 68 share their operations; the lower number is kept.
    by_operations: HashMap<Vec<SymOp>, u16>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    let settings: Vec<Setting> = (1..=530).map(load_setting).collect();
    let mut by_operations = HashMap::new();
    for setting in &settings {
        by_operations
            .entry(sorted(setting.symmetry.operations()))
            .or_insert(setting.hall_number);
    }
    Tables {
        settings,
        by_operations,
    }
});

fn load_setting(hall_number: u16) -> Setting {
    let entry = hall_symbol_entry(i32::from(hall_number)).expect("Hall numbers are 1–530");
    let hall = HallSymbol::new(entry.hall_symbol).expect("tabulated Hall symbol");
    Setting {
        hall_number,
        number: entry.number as u16,
        hm_short: entry.hm_short,
        hm_full: entry.hm_full,
        code: entry.setting,
        symmetry: Symmetry::from_group(operations(&hall).expect("tabulated translations")),
    }
}

/// The operations a Hall symbol generates. moyo lists one operation per
/// rotation; adding the centering translations completes the set. `None`
/// if a translation is no fraction with a denominator dividing 24.
fn operations(hall: &HallSymbol) -> Option<Vec<SymOp>> {
    let representatives = hall.traverse();
    let mut operations = Vec::new();
    for centering in hall.centering.lattice_points() {
        for op in &representatives {
            let translation = (op.translation + centering).map(|v| v.rem_euclid(1.0));
            operations.push(from_moyo(&Operation::new(op.rotation, translation))?);
        }
    }
    Some(operations)
}

fn from_moyo(op: &Operation) -> Option<SymOp> {
    let [x, y, z] = op.translation_as_array().map(Rational::from_decimal);
    Some(SymOp::new(op.rotation_as_array(), [x?, y?, z?]))
}

/// The operations in a fixed order, as a key for comparing sets.
fn sorted(operations: &[SymOp]) -> Vec<SymOp> {
    let mut operations = operations.to_vec();
    operations.sort_by_key(|op| {
        (
            *op.rotation(),
            op.translation().map(|t| (t.numerator(), t.denominator())),
        )
    });
    operations
}

pub(crate) fn setting(hall_number: u16) -> &'static Setting {
    &TABLES.settings[usize::from(hall_number) - 1]
}

/// The standard setting of a type.
pub(crate) fn standard(number: u16) -> &'static Setting {
    let hall_number = MoyoSetting::Standard
        .hall_number(i32::from(number))
        .expect("numbers are 1–230");
    setting(hall_number as u16)
}

/// The tabulated setting whose operations are exactly these.
pub(crate) fn match_operations(symmetry: &Symmetry) -> Option<&'static Setting> {
    let hall_number = TABLES.by_operations.get(&sorted(symmetry.operations()))?;
    Some(setting(*hall_number))
}

/// Operations from a Hall symbol such as `-P 2ybc` or `-R 3 2"c`; `None` if
/// moyo cannot parse it (it does not read change-of-basis suffixes such as
/// `(x,y,-x+z)`) or it generates no valid operation set.
pub(crate) fn hall_operations(symbol: &str) -> Option<Symmetry> {
    let hall = HallSymbol::new(&moyo_hall_symbol(symbol)?)?;
    if !generates_point_group(&hall) {
        return None;
    }
    Symmetry::new(operations(&hall)?).ok()
}

/// Whether the rotations of the generators close into a crystallographic
/// point group (at most 48 elements). With explicit axes a Hall symbol can
/// combine rotations of no common lattice, e.g. `P 3 4x`, whose group is
/// infinite; `HallSymbol::traverse` would then never return. Every element
/// is a product of at most 48 generators with row sums ≤ 2, so `i64`
/// cannot overflow.
fn generates_point_group(hall: &HallSymbol) -> bool {
    let generators: Vec<Matrix3<i64>> = hall
        .generators
        .iter()
        .map(|op| op.rotation.map(i64::from))
        .collect();
    let mut group = vec![Matrix3::identity()];
    let mut next = 0;
    while let Some(element) = group.get(next).copied() {
        for generator in &generators {
            let product = element * generator;
            if !group.contains(&product) {
                if group.len() == 48 {
                    return false;
                }
                group.push(product);
            }
        }
        next += 1;
    }
    true
}

/// A Hall symbol in moyo's spelling, checked against the grammar moyo's
/// parser assumes: it indexes and asserts rather than returning `None` on
/// an empty symbol, a lone `-`, an unknown character in an operator or a
/// non-numeric origin shift. moyo writes the axis symbols `'` and `"` of
/// International Tables B as `^` and `=`, so `R 3 2"` becomes `R 3 2=`.
fn moyo_hall_symbol(symbol: &str) -> Option<String> {
    let symbol = symbol.trim().replace('\'', "^").replace('"', "=");
    // moyo's tokens: the lattice, the operators, and from the first token
    // that opens with `(` on, the origin shift.
    let tokens: Vec<&str> = symbol.split_whitespace().collect();
    let shift_start = tokens
        .iter()
        .position(|t| t.starts_with('('))
        .unwrap_or(tokens.len());
    let (lattice, operators) = tokens[..shift_start].split_first()?;
    let lattice = lattice.strip_prefix('-').unwrap_or(lattice);
    if operators.is_empty() || !matches!(lattice, "P" | "A" | "B" | "C" | "I" | "R" | "F") {
        return None;
    }
    for token in operators {
        // <N> := "-"? <nfold> <A>? <T>?, with the axis symbol before the
        // axis, e.g. `-2^x` or `61`.
        let token = token.strip_prefix('-').unwrap_or(token);
        let token = token.strip_prefix(['1', '2', '3', '4', '6'])?;
        let token = token.strip_prefix(['^', '=']).unwrap_or(token);
        let token = token.strip_prefix(['x', 'y', 'z', '*']).unwrap_or(token);
        if !token.chars().all(|c| "123456abcnuvwd".contains(c)) {
            return None;
        }
    }
    if shift_start < tokens.len() {
        // <V> := "(" vx vy vz ")", in twelfths.
        let shift = tokens[shift_start..].join(" ");
        let shift = shift.strip_prefix('(')?.strip_suffix(')')?;
        let components: Vec<&str> = shift.split_whitespace().collect();
        if components.len() != 3 || components.iter().any(|c| c.parse::<i32>().is_err()) {
            return None;
        }
    }
    Some(symbol)
}

/// For comparing Hermann–Mauguin symbols: `P 42/m n m`, `P4_2/mnm` and
/// `p 42/M N M` all become `p42/mnm`.
fn normalize(symbol: &str) -> String {
    symbol
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

/// The setting a Hermann–Mauguin symbol names, e.g. `P 1 21/c 1`, `P 21/c`,
/// `F d -3 m :1` or `R -3 m :R`. A full symbol names its setting; a short
/// one, or a full one shared by several origin choices, means the standard
/// setting unless a `:` suffix picks another.
pub(crate) fn find_hm(symbol: &str) -> Option<&'static Setting> {
    let (name, code) = match symbol.split_once(':') {
        Some((name, code)) => (name, Some(code.trim())),
        None => (symbol, None),
    };
    let fits_code = |setting: &&Setting| {
        code.is_none_or(|code| !code.is_empty() && setting.code.starts_with(code))
    };
    let pick = |key: &str| {
        let candidates: Vec<&Setting> = TABLES
            .settings
            .iter()
            .filter(|s| normalize(s.hm_full) == key)
            .chain(
                TABLES
                    .settings
                    .iter()
                    .filter(|s| normalize(s.hm_short) == key),
            )
            .filter(fits_code)
            .collect();
        candidates
            .iter()
            .find(|s| s.is_standard())
            .or(candidates.first())
            .copied()
    };
    pick(&normalize(name)).or_else(|| {
        // A monoclinic short symbol for another setting, e.g. `P 21/a`,
        // stands for the full symbol with unique axis b, `P 1 21/a 1`.
        let name = name.trim();
        let lattice = name.chars().next().filter(char::is_ascii_uppercase)?;
        let axis = name[1..].trim();
        (!axis.is_empty() && !axis.contains(' '))
            .then(|| pick(&normalize(&format!("{lattice} 1 {axis} 1"))))
            .flatten()
    })
}

pub(crate) fn wyckoff_positions(hall_number: u16) -> impl Iterator<Item = WyckoffEntry> {
    iter_wyckoff_positions_from_hall_number(i32::from(hall_number)).map(|w| WyckoffEntry {
        multiplicity: w.multiplicity,
        letter: w.letter,
        site_symmetry: w.site_symmetry,
        coordinates: SymOp::parse(w.coordinates).expect("tabulated coordinates"),
    })
}

/// A change of basis and origin, `x' = P⁻¹·(x − p)` (design §7.8).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Transform {
    pub linear: [[f64; 3]; 3],
    pub inverse: [[f64; 3]; 3],
    pub origin: [f64; 3],
}

impl Transform {
    pub fn apply(&self, x: [f64; 3]) -> [f64; 3] {
        let d = [0, 1, 2].map(|i| x[i] - self.origin[i]);
        [0, 1, 2].map(|i| (0..3).map(|j| self.inverse[i][j] * d[j]).sum())
    }
}

/// Probe points for identifying an operation set: generic enough that
/// their orbits have exactly the symmetry of the set and no more. Three
/// distinct species keep a P1 lattice from showing its full holohedry.
const PROBES: [[f64; 3]; 3] = [
    [0.1031, 0.2357, 0.3719],
    [0.4127, 0.0813, 0.2941],
    [0.1789, 0.3511, 0.0577],
];

/// Identifies the type of an operation set that matches no tabulated
/// setting: moyo searches the symmetry of probe atoms placed by the
/// operations, in a cell whose metric `metric` (Å², invariant under the
/// operations) the operations preserve exactly. Returns the standard
/// setting and the transformation into it.
pub(crate) fn search_operations(
    symmetry: &Symmetry,
    metric: [[f64; 3]; 3],
) -> Option<(&'static Setting, Transform)> {
    let metric = Matrix3::from_fn(|i, j| metric[i][j]);
    // G = L·Lᵀ: the rows of L are basis vectors with that metric.
    let l = metric.cholesky()?.l();
    let basis = [0, 1, 2].map(|i| [0, 1, 2].map(|j| l[(i, j)]));
    let mut positions = Vec::new();
    let mut species = Vec::new();
    for (probe, number) in PROBES.iter().zip(1..) {
        for op in symmetry.operations() {
            positions.push(op.apply(*probe).map(|v| v.rem_euclid(1.0)));
            species.push(number);
        }
    }
    let dataset = dataset(basis, &positions, &species, 1e-4)?;
    if dataset.operations.len() != symmetry.operations().len() {
        return None;
    }
    let linear = dataset.std_linear;
    let inverse = linear.try_inverse()?;
    let array = |m: Matrix3<f64>| [0, 1, 2].map(|i| [0, 1, 2].map(|j| m[(i, j)]));
    let transform = Transform {
        linear: array(linear),
        inverse: array(inverse),
        origin: dataset.std_origin_shift_as_array(),
    };
    Some((setting(dataset.hall_number as u16), transform))
}

/// The symmetry moyo finds for atoms.
pub(crate) struct Search {
    pub setting: &'static Setting,
    /// Operations found in the given cell.
    pub operations: usize,
    /// Wyckoff letter of each atom, in the standard setting.
    pub wyckoffs: Vec<char>,
}

/// Searches the symmetry of atoms: `basis` holds the cell vectors as rows
/// (Å), `species` tells atoms apart, and `symprec` (Å) is how far an atom
/// may be from its symmetric image.
pub(crate) fn search_atoms(
    basis: [[f64; 3]; 3],
    positions: &[[f64; 3]],
    species: &[i32],
    symprec: f64,
) -> Option<Search> {
    let dataset = dataset(basis, positions, species, symprec)?;
    Some(Search {
        setting: setting(dataset.hall_number as u16),
        operations: dataset.operations.len(),
        wyckoffs: dataset.wyckoffs,
    })
}

fn dataset(
    basis: [[f64; 3]; 3],
    positions: &[[f64; 3]],
    species: &[i32],
    symprec: f64,
) -> Option<MoyoDataset> {
    let cell = Cell::new(
        Lattice::from_basis(basis),
        positions.iter().map(|p| Vector3::from(*p)).collect(),
        species.to_vec(),
    );
    MoyoDataset::new(
        &cell,
        symprec,
        AngleTolerance::Default,
        MoyoSetting::Standard,
        false,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tabulated_setting_is_a_group() {
        let mut orders = 0;
        for hall_number in 1..=530 {
            let setting = setting(hall_number);
            let operations = setting.symmetry.operations().to_vec();
            orders += operations.len();
            assert_eq!(
                Symmetry::new(operations),
                Ok(setting.symmetry.clone()),
                "Hall {hall_number}"
            );
            assert!(wyckoff_positions(hall_number).count() > 0);
        }
        assert_eq!(orders, 7388);
        assert_eq!(TABLES.by_operations.len(), 527);
    }

    #[test]
    fn finds_settings_by_symbol() {
        let hall = |symbol: &str| find_hm(symbol).map(|s| s.hall_number);
        assert_eq!(hall("P 42/m n m"), Some(419));
        assert_eq!(hall("P4_2/mnm"), Some(419));
        assert_eq!(hall("Pm-3m"), Some(517));
        assert_eq!(hall("F m -3 m"), Some(523));
        assert_eq!(hall("P 21/c"), Some(81));
        assert_eq!(hall("P 1 21/c 1"), Some(81));
        // `P 21/a` is unique axis b, cell choice 3.
        assert_eq!(
            find_hm("P 21/a").map(|s| (s.number, s.code)),
            Some((14, "b3"))
        );
        assert_eq!(find_hm("P 1 1 21/a").map(|s| s.code), Some("c1"));
        assert_eq!(find_hm("F d -3 m").map(|s| s.code), Some("2"));
        assert_eq!(find_hm("F d -3 m :1").map(|s| s.code), Some("1"));
        assert_eq!(find_hm("R -3 m").map(|s| s.code), Some("H"));
        assert_eq!(find_hm("R -3 m :R").map(|s| s.code), Some("R"));
        assert_eq!(hall("P 63 m c"), Some(480));
        assert_eq!(hall("Q 1"), None);
    }

    #[test]
    fn parses_hall_symbols() {
        let ops = |symbol: &str| hall_operations(symbol).map(|s| s.operations().len());
        assert_eq!(ops("-P 2ybc"), Some(4));
        assert_eq!(ops("-F 4 2 3"), Some(192));
        assert_eq!(ops("P 31 2 (0 0 4)"), Some(6));
        assert_eq!(ops("not a Hall symbol"), None);
        for hall_number in 1..=530 {
            let entry = hall_symbol_entry(i32::from(hall_number)).unwrap();
            assert_eq!(
                hall_operations(entry.hall_symbol).as_ref(),
                Some(&setting(hall_number).symmetry),
                "{}",
                entry.hall_symbol
            );
        }
    }

    #[test]
    fn reads_axis_symbols_of_international_tables() {
        // COD writes `"` where moyo's table has `=` (No. 155 and 167,
        // hexagonal axes).
        let r32 = hall_operations("R 3 2\"").expect("R 3 2\"");
        assert_eq!(r32, setting(444).symmetry);
        let r3c = hall_operations("-R 3 2\"c").expect("-R 3 2\"c");
        assert_eq!(match_operations(&r3c).map(|s| s.number), Some(167));
        assert_eq!(
            hall_operations("P 3 2'").map(|s| s.operations().len()),
            Some(6)
        );
    }

    #[test]
    fn rejects_malformed_hall_symbols() {
        // moyo panics on these...
        for symbol in [
            "",
            "-",
            "P -",
            "P 2#",
            "P 2 2\"#",
            "P 2′",
            "P 2(0 0 1)",
            "-P 2ybc (x, y, z)",
            "P 2 (0 0 x)",
            // ...and reads these in part, ignoring the rest.
            "P",
            "P1",
            "P 2 (0 0 1",
        ] {
            assert_eq!(hall_operations(symbol), None, "{symbol:?}");
        }
        // Parses, but 3z of a hexagonal cell and 4x of a square one
        // generate an infinite group.
        assert_eq!(hall_operations("P 3 4x"), None);
    }
}
