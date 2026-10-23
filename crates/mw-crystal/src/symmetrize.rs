//! Symmetry detection and symmetrization (design §7.10). [`DetectSymmetry`]
//! finds the symmetry the atoms of a structure have, with moyo, and builds
//! the structure it describes: every atom averaged onto its symmetric
//! position, the equivalent atoms merged into one site, occupancies and
//! displacement parameters averaged the same way. The cell is kept, or with
//! `standardize` the result is in the conventional cell of the standard
//! setting. [`crate::Edit::Symmetrize`] commits the same result.
//!
//! The work happens in a frame: the structure's own cell, or the standard
//! conventional cell that moyo gives. The atoms are taken into it, grouped
//! into orbits by the frame's exact operations, and each orbit's
//! representative is the average of what every operation brings back to
//! it. moyo's operations and origin are only approximate; the operations
//! come from the tables, carried into the frame exactly.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use crate::cell_transform::{self, Exact, Matrix};
use crate::expand::wrap;
use crate::id_map::PositionMap;
use crate::lattice::norm;
use crate::tables::{self, Found};
use crate::transform::{self, tidy, transform_adp, transform_su};
use crate::{
    Adp, CellParam, CellParams, CellTransform, IdMap, Lattice, Measured, Occupant, Orientation,
    Rational, Site, SiteId, SpaceGroup, SpaceGroupType, Structure, SymOp, Symmetry, constraints,
    expand, linalg,
};

/// The most operations a symmetrized structure may keep in its own cell: a
/// supercell holds the symmetry once per lattice point, and every later
/// expansion and identification grows with it. Fm-3m's in a cell eight
/// times its conventional one.
pub const MAX_OPERATIONS: usize = 1536;

/// The most atoms the conventional cell may hold when standardizing.
const MAX_ATOMS: usize = 20_000;

/// How far, in multiples of the tolerance, an atom may lie from where an
/// operation puts the atom it is equivalent to: the atoms themselves may be
/// off by the tolerance, and the operations' origin, which moyo finds from
/// them, by as much again.
const REACH: f64 = 4.0;

/// The largest denominator a change of cell into the standard setting may
/// have: a supercell of that many cells along an axis.
const MAX_DENOMINATOR: i128 = 1200;

/// How much a value may differ from another and still count as the same,
/// relative to its size.
const SAME: f64 = 1e-12;

/// Searches the symmetry of a structure's atoms and symmetrizes by it
/// (design §7.10). Runs on a snapshot, so it can run in the background.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectSymmetry {
    /// Å: how far an atom may be from its symmetric image.
    pub symprec: f64,
    /// Put the result in the conventional cell of the standard setting,
    /// rather than keeping the cell.
    pub standardize: bool,
}

/// What symmetrizing a structure gives: the preview of design §7.10, and
/// the result [`crate::Edit::Symmetrize`] commits.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Symmetrization {
    /// The symmetrized structure.
    pub structure: Structure,
    /// The type moyo finds for the atoms: International Tables number.
    pub detected_number: u16,
    /// Its short Hermann–Mauguin symbol, e.g. `P m -3 m`.
    pub detected_symbol: &'static str,
    /// The type of the result's operations. Keeping a cell that cannot
    /// hold the whole symmetry (a 2a × a × a cell of a cubic crystal) gives
    /// a subgroup of the detected type; standardizing gives all of it.
    pub space_group: Option<SpaceGroupType>,
    /// Å: the largest distance an atom moves, the change of the cell
    /// included and a shift of the origin not.
    pub max_displacement: f64,
    /// The site of the result whose atoms moved most.
    pub moved_most: Option<SiteId>,
    /// Whether the basis changed: the conventional cell differs from the
    /// structure's.
    pub cell_changed: bool,
    /// Whether the cell lies differently in Cartesian space.
    pub orientation_changed: bool,
    /// Å: how far the origin moved. 0 when the structure's origin is, within
    /// the tolerance, an origin of the symmetry.
    pub origin_shift: f64,
    /// From the structure's fractional coordinates to the result's.
    pub(crate) mapping: PositionMap,
}

impl Symmetrization {
    /// What became of the atoms of `before`, the structure symmetrized: each
    /// is the nearest atom of the result (design §7.11).
    pub fn ids(&self, before: &Structure) -> IdMap {
        IdMap::relocated(before, &self.structure, self.mapping)
    }
}

/// Why a structure cannot be symmetrized.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SymmetrizeError {
    /// The tolerance is not a positive number.
    InvalidTolerance,
    NoAtoms,
    /// The search found no symmetry: atoms of the same kind overlap, or the
    /// tolerance is too large for the cell.
    NotFound,
    /// The atoms have no symmetry beyond what the structure already has
    /// (only P1, for a structure in P1).
    NoHigherSymmetry,
    /// The atoms do not follow the operations found within the tolerance;
    /// another tolerance may settle it.
    Mismatch,
    /// Atoms that the search keeps apart lie closer than the structure's
    /// position tolerance, which would merge them: the tolerance is below
    /// the position tolerance.
    PositionTolerance,
    /// The result would have too many operations or atoms.
    TooLarge,
}

impl SymmetrizeError {
    /// A stable identifier for programs and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidTolerance => "symmetrize_invalid_tolerance",
            Self::NoAtoms => "symmetrize_no_atoms",
            Self::NotFound => "symmetrize_not_found",
            Self::NoHigherSymmetry => "symmetrize_no_higher_symmetry",
            Self::Mismatch => "symmetrize_mismatch",
            Self::PositionTolerance => "symmetrize_position_tolerance",
            Self::TooLarge => "symmetrize_too_large",
        }
    }
}

/// English, for programs, logs and AI callers.
impl fmt::Display for SymmetrizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidTolerance => "the symmetry tolerance must be a positive number of Å",
            Self::NoAtoms => "the structure has no atoms",
            Self::NotFound => {
                "no symmetry was found: atoms of the same kind may overlap, or the tolerance be \
                 too large for the cell"
            }
            Self::NoHigherSymmetry => {
                "the atoms have no symmetry beyond what the structure already has"
            }
            Self::Mismatch => {
                "the atoms do not follow the symmetry found within the tolerance; try another \
                 tolerance"
            }
            Self::PositionTolerance => {
                "atoms the symmetry keeps apart lie closer than the position tolerance; raise \
                 the symmetry tolerance to at least the position tolerance"
            }
            Self::TooLarge => {
                "the result would have too many symmetry operations or atoms; standardize to the \
                 conventional cell, or symmetrize a smaller cell"
            }
        })
    }
}

impl std::error::Error for SymmetrizeError {}

impl DetectSymmetry {
    /// With the structure's own tolerance, keeping the cell.
    pub fn new(structure: &Structure) -> Self {
        Self {
            symprec: structure.tolerances.symprec,
            standardize: false,
        }
    }

    pub fn run(&self, structure: &Structure) -> Result<Symmetrization, SymmetrizeError> {
        let symprec = self.symprec;
        if !(symprec.is_finite() && symprec > 0.0) {
            return Err(SymmetrizeError::InvalidTolerance);
        }
        // One site per atom: the operations to find are the atoms' own.
        let expanded;
        let p1 = if structure.symmetry.is_p1() {
            structure
        } else {
            expanded = transform::expand_to_p1(structure).structure;
            &expanded
        };
        if p1.sites.is_empty() {
            return Err(SymmetrizeError::NoAtoms);
        }
        // A left-handed lattice holds the mirror image of what its
        // coordinates describe on a right-handed basis (design §4.2): the
        // work happens on −a, −b, −c, and a kept cell goes back after.
        let left = p1.lattice.is_left_handed();
        let flipped;
        let input = if left {
            flipped = transform::inverted(p1);
            &flipped
        } else {
            p1
        };
        let kinds = kinds(input);
        let positions: Vec<[f64; 3]> = input.sites.iter().map(|s| wrap(s.position)).collect();
        let found = tables::find_symmetry(
            input.lattice.params().standard_matrix(),
            &positions,
            &kinds,
            symprec,
        )
        .ok_or(SymmetrizeError::NotFound)?;
        if found.setting.number == 1 {
            return Err(SymmetrizeError::NoHigherSymmetry);
        }
        let frame = Frame::new(input, &found, symprec, self.standardize)?;
        if !self.standardize
            && frame.symmetry.operations().len() <= structure.symmetry.operations().len()
        {
            return Err(SymmetrizeError::NoHigherSymmetry);
        }
        let built = frame.build(input, &kinds, symprec)?;

        let mut mapping = if left {
            inversion()
        } else {
            PositionMap::IDENTITY
        };
        mapping = mapping.then(&frame.mapping);
        let mut result = built.structure;
        if self.standardize {
            // Of the conventional cells that give the same operations, the
            // one M8's presets pick: closest to the standard orientation,
            // then the shortest origin shift (design §7.8).
            let canonical =
                cell_transform::canonical(&result, CellTransform::IDENTITY, &result.symmetry, true);
            if let Some(change) = canonical.filter(|c| !c.is_identity()) {
                result = transform::change_basis(&result, &change)
                    .map_err(|_| SymmetrizeError::TooLarge)?;
                mapping = mapping.then(&change_map(&change));
            }
            // A new cell: the sites in it, as the P1 expansion puts them.
            for site in &mut result.sites {
                site.position = wrap(site.position).map(tidy);
            }
        } else if left {
            result = transform::inverted(&result);
            for site in &mut result.sites {
                site.position = site.position.map(tidy);
            }
            mapping = mapping.then(&inversion());
        }
        // Of −a, −b, −c for a left-handed lattice, as the derived data do.
        let space_group = SpaceGroupType::identify(
            &transform::right_handed(&result).symmetry,
            result.lattice.params(),
        );
        result.space_group = space_group.as_ref().map_or_else(SpaceGroup::default, |sg| {
            SpaceGroup::new(Some(sg.hm_symbol.into()), Some(sg.number))
        });

        let origin_shift = origin_shift(structure, &mapping);
        let cell_changed = self.standardize && !is_identity(&mapping);
        let orientation_changed = !same_orientation(
            structure.lattice.orientation(),
            result.lattice.orientation(),
        );
        Ok(Symmetrization {
            structure: result,
            detected_number: found.setting.number,
            detected_symbol: found.setting.hm_short,
            space_group,
            max_displacement: built.max_displacement,
            moved_most: built.moved_most,
            cell_changed,
            orientation_changed,
            origin_shift,
            mapping,
        })
    }
}

/// `x' = −x`: the coordinates on the basis −a, −b, −c.
fn inversion() -> PositionMap {
    PositionMap::new(
        [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]],
        [0.0; 3],
    )
}

/// Tells atoms apart for the search: their occupants' elements, oxidation
/// states and occupancies, and their disorder group. Atoms of one kind may
/// be equivalent.
pub(crate) fn kind_key(site: &Site) -> String {
    let mut key = site
        .occupants
        .iter()
        .map(|o| {
            let oxidation = o
                .species
                .oxidation
                .map_or_else(String::new, |v| v.to_string());
            format!(
                "{}{oxidation}:{:.3}",
                o.species.element,
                o.occupancy.value()
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    if let Some(disorder) = &site.disorder {
        key.push_str(&format!(
            "|{}/{}",
            disorder.assembly.as_deref().unwrap_or(""),
            disorder.group
        ));
    }
    key
}

/// A number for each site's kind.
fn kinds(structure: &Structure) -> Vec<i32> {
    let mut keys: Vec<String> = Vec::new();
    structure
        .sites
        .iter()
        .map(|site| {
            let key = kind_key(site);
            let index = keys.iter().position(|k| *k == key).unwrap_or_else(|| {
                keys.push(key);
                keys.len() - 1
            });
            index as i32
        })
        .collect()
}

/// The operations' rotations as exact matrices.
fn exact_rotation(op: &SymOp) -> Matrix {
    op.rotation()
        .map(|row| row.map(|v| Exact::integer(v.into())))
}

fn exact_vector(v: &[Rational; 3]) -> [Exact; 3] {
    v.map(Exact::of)
}

fn to_f64(m: &Matrix) -> [[f64; 3]; 3] {
    m.map(|row| row.map(Exact::to_f64))
}

/// The fraction with the smallest denominator within rounding of `value`.
fn fraction(value: f64) -> Option<Exact> {
    (1..=MAX_DENOMINATOR).find_map(|den| {
        let scaled = value * den as f64;
        let num = scaled.round();
        ((scaled - num).abs() < 1e-8 * (den as f64).max(1.0))
            .then(|| Exact::new(num as i128, den))
            .flatten()
    })
}

/// Adds two vectors modulo 1.
fn add_mod1(a: &[Exact; 3], b: &[Exact; 3]) -> Option<[Exact; 3]> {
    Some([
        a[0].add(b[0])?.fract(),
        a[1].add(b[1])?.fract(),
        a[2].add(b[2])?.fract(),
    ])
}

/// The group modulo 1 that `generators` generate, starting from 0; `None`
/// when it grows beyond `limit` elements or its numbers overflow.
fn translation_group(generators: &[[Exact; 3]], limit: usize) -> Option<Vec<[Exact; 3]>> {
    let mut group = vec![[Exact::ZERO; 3]];
    let mut next = 0;
    while next < group.len() {
        let t = group[next];
        for g in generators {
            let sum = add_mod1(&t, g)?;
            if !group.contains(&sum) {
                if group.len() >= limit {
                    return None;
                }
                group.push(sum);
            }
        }
        next += 1;
    }
    Some(group)
}

/// The cell the work happens in, with its exact operations, and how the
/// structure's coordinates map into it.
struct Frame {
    symmetry: Symmetry,
    /// Fractional coordinates in the frame of a position `x` of the
    /// structure (on the right-handed basis): `linear·(x − origin)`.
    mapping: PositionMap,
    /// Lattice translations of the structure, in the frame and modulo 1,
    /// that take its atoms to the other atoms of the frame's cell.
    images: Vec<[f64; 3]>,
    /// The frame's basis vectors in Cartesian Å before the cell is
    /// symmetrized, rows a, b, c.
    basis: [[f64; 3]; 3],
    /// The symmetrized lattice of the frame.
    lattice: Lattice,
    /// The linear part of `mapping`, for displacement parameters and
    /// uncertainties.
    linear: [[f64; 3]; 3],
    /// `true` for the structure's own cell: the lattice keeps the cell
    /// parameters that do not change, with their uncertainties.
    kept: bool,
}

impl Frame {
    fn new(
        input: &Structure,
        found: &Found,
        symprec: f64,
        standardize: bool,
    ) -> Result<Self, SymmetrizeError> {
        let too_large = || SymmetrizeError::TooLarge;
        let transform = &found.transform;
        let mut p = [[Exact::ZERO; 3]; 3];
        for (row, linear) in p.iter_mut().zip(&transform.linear) {
            for (entry, value) in row.iter_mut().zip(linear) {
                *entry = fraction(*value).ok_or_else(too_large)?;
            }
        }
        let inverse = cell_transform::mat_inverse(&p).ok_or_else(too_large)?;
        let standard = &found.setting.symmetry;
        let operations: HashSet<SymOp> = standard.operations().iter().copied().collect();

        // The standard operations whose rotations are integer in the
        // structure's basis, `P·W·P⁻¹`: all of them in a cell that can hold
        // the symmetry; a subgroup in one that cannot.
        let mut kept: Vec<(&SymOp, [[i32; 3]; 3])> = Vec::new();
        for op in standard.operations() {
            let w = cell_transform::mat_mul(&p, &exact_rotation(op)).ok_or_else(too_large)?;
            let w = cell_transform::mat_mul(&w, &inverse).ok_or_else(too_large)?;
            if w.iter().flatten().all(|v| v.is_integer()) {
                let integer = w.map(|row| row.map(|v| v.to_f64().round() as i32));
                kept.push((op, integer));
            }
        }
        let frame_ops: Vec<&SymOp> = if standardize {
            standard.operations().iter().collect()
        } else {
            kept.iter().map(|(op, _)| *op).collect()
        };

        // The lattice translations of the result in the structure's basis:
        // the standard cell's basis vectors and centering, `P·t`.
        let mut generators: Vec<[Exact; 3]> =
            (0..3).map(|j| [0, 1, 2].map(|i| p[i][j].fract())).collect();
        for op in standard.operations() {
            if op.rotation() == SymOp::IDENTITY.rotation() && !op.is_identity() {
                let t = cell_transform::mat_apply(&p, &exact_vector(op.translation()))
                    .ok_or_else(too_large)?;
                generators.push(t.map(Exact::fract));
            }
        }
        let translations = translation_group(&generators, MAX_OPERATIONS).ok_or_else(too_large)?;

        // Directions along which every rotation leaves the origin free (a
        // polar axis), in the structure's basis.
        let rows: Vec<[Rational; 3]> = frame_ops
            .iter()
            .flat_map(|op| {
                let w = *op.rotation();
                (0..3).map(move |i| {
                    [0, 1, 2].map(|j| Rational::integer(i64::from(w[i][j] - i32::from(i == j))))
                })
            })
            .collect();
        let p_f64 = to_f64(&p);
        let polar: Vec<[f64; 3]> = linalg::null_space(&rows)
            .into_iter()
            .map(|v| {
                let v = v.map(Rational::to_f64);
                [0, 1, 2].map(|i| (0..3).map(|j| p_f64[i][j] * v[j]).sum())
            })
            .collect();

        let metric = input.lattice.params().metric();
        let shifts: Vec<[f64; 3]> = translations.iter().map(|t| t.map(Exact::to_f64)).collect();
        let origin = Origin {
            metric,
            translations: shifts,
            projector: projector(&polar, &metric),
        };
        let found_origin = transform.origin;
        // Standardizing keeps the tabulated operations, so the origin may
        // only move to another origin of the same operations; the
        // structure's own cell may describe them from any origin.
        let normal = |q: &[Rational; 3]| {
            frame_ops.iter().all(|op| {
                let w = op.rotation();
                let shift = [0, 1, 2].map(|i| {
                    (0..3).fold(op.translation()[i] + q[i], |sum, j| {
                        sum - q[j] * i64::from(w[i][j])
                    })
                });
                operations.contains(&SymOp::new(*w, shift))
            })
        };
        let offset = |q: &[Rational; 3]| {
            let q = q.map(Rational::to_f64);
            [0, 1, 2].map(|i| found_origin[i] - (0..3).map(|j| p_f64[i][j] * q[j]).sum::<f64>())
        };
        // Any origin in 24ths of the cell for the structure's own cell; for
        // the tabulated operations, the shifts of 8ths and 6ths, which hold
        // the translations of every space group's normalizer (design §7.8).
        let fine = grid(&(0..24).map(|k| Rational::new(k, 24)).collect::<Vec<_>>());
        let normalizer = {
            let mut steps: Vec<Rational> = Vec::new();
            for den in [8, 6] {
                for num in 0..den {
                    let step = Rational::new(num, den);
                    if !steps.contains(&step) {
                        steps.push(step);
                    }
                }
            }
            grid(&steps)
        };
        let best = |candidates: &mut dyn Iterator<Item = &[Rational; 3]>| {
            candidates
                .map(|q| {
                    let (shift, length) = origin.reduce(offset(q));
                    (length, *q, shift)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
        };
        let normal_best = || best(&mut normalizer.iter().filter(|q| normal(q)));
        let near = if standardize {
            normal_best()
        } else {
            best(&mut fine.iter())
        };
        let (q, shift) = match near {
            // The structure's origin is an origin of the symmetry, within
            // the tolerance: it stays, and the atoms settle around it.
            Some((length, q, _)) if length <= symprec => (q, [0.0; 3]),
            // Otherwise the origin moves to the nearest origin of the
            // tabulated operations.
            near => {
                let (_, q, shift) = if standardize { near } else { normal_best() }
                    .ok_or(SymmetrizeError::NotFound)?;
                (q, shift)
            }
        };

        let exact_q = exact_vector(&q);
        let (symmetry, linear, images, basis, frame_metric) = if standardize {
            // The tabulated operations; atoms come in through P⁻¹ and every
            // lattice translation of the structure.
            let linear = to_f64(&inverse);
            let generators: Vec<[Exact; 3]> = (0..3)
                .map(|j| [0, 1, 2].map(|i| inverse[i][j].fract()))
                .collect();
            let images = translation_group(&generators, MAX_OPERATIONS).ok_or_else(too_large)?;
            if images.len() * input.sites.len() > MAX_ATOMS {
                return Err(too_large());
            }
            let old = input.lattice.matrix();
            let basis =
                [0, 1, 2].map(|j| [0, 1, 2].map(|k| (0..3).map(|i| p_f64[i][j] * old[i][k]).sum()));
            let g = input.lattice.params().metric();
            let frame_metric = [0, 1, 2].map(|i| {
                [0, 1, 2].map(|j| {
                    let mut sum = 0.0;
                    for a in 0..3 {
                        for b in 0..3 {
                            sum += p_f64[a][i] * g[a][b] * p_f64[b][j];
                        }
                    }
                    sum
                })
            });
            (
                standard.clone(),
                linear,
                images
                    .iter()
                    .map(|t| t.map(Exact::to_f64))
                    .collect::<Vec<_>>(),
                basis,
                frame_metric,
            )
        } else {
            // `P·W·P⁻¹` with the translation `P·(w + (I − W)·q)`, once per
            // lattice translation.
            let mut seen = HashSet::new();
            let mut list = Vec::new();
            for t in &translations {
                for (op, w) in &kept {
                    let rotation = exact_rotation(op);
                    let wq =
                        cell_transform::mat_apply(&rotation, &exact_q).ok_or_else(too_large)?;
                    let mut shift = [Exact::ZERO; 3];
                    for i in 0..3 {
                        shift[i] = Exact::of(op.translation()[i])
                            .add(exact_q[i])
                            .and_then(|v| v.sub(wq[i]))
                            .ok_or_else(too_large)?;
                    }
                    let moved = cell_transform::mat_apply(&p, &shift).ok_or_else(too_large)?;
                    let translation = add_mod1(&moved, t).ok_or_else(too_large)?;
                    let rational = |v: Exact| v.to_rational().ok_or_else(too_large);
                    let op = SymOp::new(
                        *w,
                        [
                            rational(translation[0])?,
                            rational(translation[1])?,
                            rational(translation[2])?,
                        ],
                    );
                    if seen.insert(op) {
                        if list.len() >= MAX_OPERATIONS {
                            return Err(too_large());
                        }
                        list.push(op);
                    }
                }
            }
            let symmetry = Symmetry::from_group(list);
            let symmetry = match tables::match_operations(&symmetry) {
                Some(setting) => setting.symmetry.clone(),
                None => symmetry,
            };
            (
                symmetry,
                [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                vec![[0.0; 3]],
                input.lattice.matrix(),
                input.lattice.params().metric(),
            )
        };

        let symmetrized = constraints::symmetrized_metric(&symmetry, frame_metric);
        let computed = CellParams::from_metric(symmetrized);
        let lattice = if standardize {
            let params = tidy_params(&computed, None);
            Lattice::oriented_as(params, basis).ok_or_else(too_large)?
        } else {
            let params = tidy_params(&computed, Some(input.lattice.params()));
            Lattice::with_orientation(params, *input.lattice.orientation())
        };
        if !lattice.params().is_valid() {
            return Err(too_large());
        }
        let shift_in_frame =
            [0, 1, 2].map(|i| -(0..3).map(|j| linear[i][j] * shift[j]).sum::<f64>());
        Ok(Self {
            symmetry,
            mapping: PositionMap::new(linear, shift_in_frame),
            images,
            basis,
            lattice,
            linear,
            kept: !standardize,
        })
    }
}

/// Every point whose coordinates are among `steps`.
fn grid(steps: &[Rational]) -> Vec<[Rational; 3]> {
    let mut grid = Vec::with_capacity(steps.len().pow(3));
    for &x in steps {
        for &y in steps {
            for &z in steps {
                grid.push([x, y, z]);
            }
        }
    }
    grid
}

/// Finds the shortest description of an origin shift.
struct Origin {
    metric: [[f64; 3]; 3],
    /// Lattice translations of the result in the structure's basis, which
    /// move the origin to an equivalent one.
    translations: Vec<[f64; 3]>,
    /// Removes the components along which the origin is free.
    projector: [[f64; 3]; 3],
}

impl Origin {
    /// The shortest equivalent of the shift and its length in Å.
    fn reduce(&self, shift: [f64; 3]) -> ([f64; 3], f64) {
        let mut best = (shift, f64::INFINITY);
        for t in &self.translations {
            let v = [0, 1, 2].map(|i| {
                let d = shift[i] - t[i];
                d - d.round()
            });
            let v = [0, 1, 2].map(|i| (0..3).map(|j| self.projector[i][j] * v[j]).sum::<f64>());
            let length = norm(&self.metric, v);
            if length < best.1 {
                best = (v, length);
            }
        }
        best
    }
}

/// `I − U·(UᵀGU)⁻¹·UᵀG`: removes the components along the directions `U`,
/// orthogonally in the metric `G`.
fn projector(directions: &[[f64; 3]], metric: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    if directions.is_empty() {
        return identity;
    }
    let k = directions.len();
    let u = nalgebra::DMatrix::from_fn(3, k, |i, j| directions[j][i]);
    let g = nalgebra::DMatrix::from_fn(3, 3, |i, j| metric[i][j]);
    let gram = u.transpose() * &g * &u;
    let Some(inverse) = gram.try_inverse() else {
        return identity;
    };
    let removed = &u * inverse * u.transpose() * g;
    [0, 1, 2].map(|i| [0, 1, 2].map(|j| identity[i][j] - removed[(i, j)]))
}

/// Cell parameters computed from a symmetrized metric, put on the numbers
/// they are within rounding of; with `old`, a parameter that did not change
/// keeps its value and uncertainty.
fn tidy_params(computed: &CellParams, old: Option<&CellParams>) -> CellParams {
    let mut params = *computed;
    for param in CellParam::ALL {
        let value = computed.get(param).value();
        let kept = old
            .map(|old| old.get(param))
            .filter(|old| (old.value() - value).abs() <= SAME * old.value().abs().max(1.0));
        params.set(param, kept.unwrap_or(Measured::exact(tidy(value))));
    }
    params
}

/// The inverse of an operation: `(W⁻¹, −W⁻¹·w)`.
fn inverse(op: &SymOp) -> SymOp {
    let m = op.rotation();
    let det = op.determinant();
    let c = |i: usize, j: usize| {
        let (r0, r1) = ((i + 1) % 3, (i + 2) % 3);
        let (c0, c1) = ((j + 1) % 3, (j + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    // adj(W)/det W, with det W = ±1.
    let w = [0, 1, 2].map(|i| [0, 1, 2].map(|j| c(j, i) * det));
    let t = op.translation();
    let shift =
        [0, 1, 2].map(|i| (0..3).fold(Rational::ZERO, |sum, j| sum - t[j] * i64::from(w[i][j])));
    SymOp::new(w, shift)
}

/// One atom of the structure in the frame: a site's position taken
/// through the frame's mapping, and through a lattice translation when the
/// frame's cell holds several images of it.
struct Observation {
    kind: i32,
    /// Fractional coordinates in the frame.
    position: [f64; 3],
    /// Index of the structure's site.
    source: usize,
    position_su: [Option<f64>; 3],
    /// With displacement parameters in the frame.
    occupants: Vec<Occupant>,
}

/// Several observations of one atom of the frame's cell: one, unless the
/// frame's cell is smaller than the structure's.
struct Cluster {
    kind: i32,
    /// The first observation's position.
    position: [f64; 3],
    observations: Vec<usize>,
}

/// The result of averaging in the frame.
struct Built {
    structure: Structure,
    max_displacement: f64,
    moved_most: Option<SiteId>,
}

/// One orbit of the frame: its representative cluster, and for each
/// operation the cluster it brings the representative to, with the lattice
/// translation that brings that cluster's position next to the image.
struct FrameOrbit {
    representative: usize,
    images: Vec<(usize, [f64; 3])>,
}

impl Frame {
    fn build(
        &self,
        input: &Structure,
        kinds: &[i32],
        symprec: f64,
    ) -> Result<Built, SymmetrizeError> {
        let reach = REACH * symprec;
        let metric = self.lattice.params().metric();
        let from = input.lattice.params().reciprocal_lengths();
        let to = self.lattice.params().reciprocal_lengths();
        let distance = |a: [f64; 3], b: [f64; 3]| {
            norm(
                &metric,
                [0, 1, 2].map(|i| {
                    let d = a[i] - b[i];
                    d - d.round()
                }),
            )
        };

        let mut observations = Vec::new();
        for (source, site) in input.sites.iter().enumerate() {
            let base = self.mapping.apply(site.position);
            let occupants: Vec<Occupant> = site
                .occupants
                .iter()
                .map(|occupant| Occupant {
                    adp: occupant.adp.map(|adp| {
                        if self.kept {
                            adp
                        } else {
                            transform_adp(adp, &self.linear, &from, &to)
                        }
                    }),
                    ..occupant.clone()
                })
                .collect();
            let position_su = if self.kept {
                site.position_su
            } else {
                transform_su(site.position_su, &self.linear)
            };
            for image in &self.images {
                let position = [0, 1, 2].map(|i| base[i] + image[i]);
                observations.push(Observation {
                    kind: kinds[source],
                    position: if self.kept { position } else { wrap(position) },
                    source,
                    position_su,
                    occupants: occupants.clone(),
                });
            }
        }

        // Observations of the same atom of the frame's cell, which a smaller
        // conventional cell brings together.
        let mut clusters: Vec<Cluster> = Vec::new();
        for (index, observation) in observations.iter().enumerate() {
            let same = (!self.kept)
                .then(|| {
                    clusters.iter().position(|c| {
                        c.kind == observation.kind
                            && distance(c.position, observation.position) <= reach
                    })
                })
                .flatten();
            match same {
                Some(c) => clusters[c].observations.push(index),
                None => clusters.push(Cluster {
                    kind: observation.kind,
                    position: observation.position,
                    observations: vec![index],
                }),
            }
        }
        // A cluster's position: the mean of its observations.
        let mean_position = |cluster: &Cluster| {
            let first = cluster.position;
            let mut sum = [0.0; 3];
            for &o in &cluster.observations {
                let p = observations[o].position;
                for i in 0..3 {
                    sum[i] += p[i] - (p[i] - first[i]).round();
                }
            }
            sum.map(|v| v / cluster.observations.len() as f64)
        };
        let centers: Vec<[f64; 3]> = clusters.iter().map(mean_position).collect();

        // Orbits under the frame's operations.
        let operations = self.symmetry.operations();
        let mut orbit_of: Vec<Option<usize>> = vec![None; clusters.len()];
        let mut orbits: Vec<FrameOrbit> = Vec::new();
        for c in 0..clusters.len() {
            if orbit_of[c].is_some() {
                continue;
            }
            let index = orbits.len();
            let x = centers[c];
            let mut images = Vec::with_capacity(operations.len());
            for op in operations {
                let y = op.apply(x);
                let nearest = clusters
                    .iter()
                    .enumerate()
                    .filter(|(_, k)| k.kind == clusters[c].kind)
                    .map(|(k, _)| (k, distance(y, centers[k])))
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                let Some((k, _)) = nearest.filter(|(_, d)| *d <= reach) else {
                    return Err(SymmetrizeError::Mismatch);
                };
                match orbit_of[k] {
                    Some(other) if other != index => return Err(SymmetrizeError::Mismatch),
                    _ => orbit_of[k] = Some(index),
                }
                let shift = [0, 1, 2].map(|i| (y[i] - centers[k][i]).round());
                images.push((k, shift));
            }
            orbit_of[c] = Some(index);
            // Every member is reached by as many operations as leave the
            // representative in place.
            let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
            for (k, _) in &images {
                *counts.entry(*k).or_default() += 1;
            }
            let stabilizer = counts.get(&c).copied().unwrap_or(0);
            if stabilizer == 0 || counts.values().any(|n| *n != stabilizer) {
                return Err(SymmetrizeError::Mismatch);
            }
            orbits.push(FrameOrbit {
                representative: c,
                images,
            });
        }

        let inverses: Vec<SymOp> = operations.iter().map(inverse).collect();
        let rotations: Vec<[[f64; 3]; 3]> = inverses
            .iter()
            .map(|op| op.rotation().map(|row| row.map(f64::from)))
            .collect();
        let new_basis = self.lattice.matrix();
        let cartesian = |basis: &[[f64; 3]; 3], x: [f64; 3]| {
            [0, 1, 2].map(|k| (0..3).map(|i| x[i] * basis[i][k]).sum::<f64>())
        };

        let mut sites = Vec::with_capacity(orbits.len());
        let mut max_displacement = 0.0;
        let mut moved_most = None;
        let mut sizes = Vec::with_capacity(orbits.len());
        for orbit in &orbits {
            let c = orbit.representative;
            let x = centers[c];
            // Each operation brings its member back onto the representative.
            let mut sum = [0.0; 3];
            for (g, (k, shift)) in orbit.images.iter().enumerate() {
                let member = [0, 1, 2].map(|i| centers[*k][i] + shift[i]);
                let back = inverses[g].apply(member);
                for i in 0..3 {
                    sum[i] += back[i] - (back[i] - x[i]).round();
                }
            }
            let averaged = sum.map(|v| tidy(v / orbit.images.len() as f64));
            let first = &observations[clusters[c].observations[0]];
            let position_su = [0, 1, 2].map(|i| {
                (averaged[i] == first.position[i])
                    .then_some(first.position_su[i])
                    .flatten()
            });

            // Every observation of the orbit, with the operation that
            // brings it back to the representative.
            let mut contributions: Vec<(usize, &Observation)> = Vec::new();
            for (g, (k, _)) in orbit.images.iter().enumerate() {
                for &o in &clusters[*k].observations {
                    contributions.push((g, &observations[o]));
                }
            }
            let occupants = first
                .occupants
                .iter()
                .enumerate()
                .map(|(i, occupant)| Occupant {
                    occupancy: averaged_occupancy(&contributions, i, occupant.occupancy),
                    adp: occupant
                        .adp
                        .map(|adp| averaged_adp(&contributions, &rotations, i, adp, &to)),
                    ..occupant.clone()
                })
                .collect();
            let source = &input.sites[first.source];
            let id = source.id;
            sites.push(Site {
                id,
                position: averaged,
                position_su,
                occupants,
                disorder: source.disorder.clone(),
            });
            let members: HashSet<usize> = orbit.images.iter().map(|(k, _)| *k).collect();
            sizes.push((id, members.len()));

            // How far each observation moved.
            for (g, (k, _)) in orbit.images.iter().enumerate() {
                let image = operations[g].apply(averaged);
                for &o in &clusters[*k].observations {
                    let old = observations[o].position;
                    let new = [0, 1, 2].map(|i| image[i] - (image[i] - old[i]).round());
                    let a = cartesian(&new_basis, new);
                    let b = cartesian(&self.basis, old);
                    let d = (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt();
                    if d > max_displacement {
                        max_displacement = d;
                        moved_most = Some(id);
                    }
                }
            }
        }
        merge_labels(&mut sites, &orbits, &clusters, &observations);

        let structure = Structure {
            lattice: self.lattice,
            symmetry: self.symmetry.clone(),
            space_group: SpaceGroup::default(),
            sites,
            tolerances: input.tolerances,
        };
        // The position tolerance must see the same atoms.
        for orbit in expand::orbits(&structure) {
            let expected = sizes.iter().find(|(id, _)| *id == orbit.site).map(|s| s.1);
            if expected != Some(orbit.positions.len()) {
                return Err(SymmetrizeError::PositionTolerance);
            }
        }
        Ok(Built {
            structure,
            max_displacement,
            moved_most,
        })
    }
}

/// The occupancy of occupant `index` over the orbit: the representative's
/// own when all agree, else the mean, which has no uncertainty.
fn averaged_occupancy(
    contributions: &[(usize, &Observation)],
    index: usize,
    own: Measured,
) -> Measured {
    let values: Vec<f64> = contributions
        .iter()
        .filter_map(|(_, o)| o.occupants.get(index).map(|o| o.occupancy.value()))
        .collect();
    if values.iter().all(|v| *v == own.value()) {
        own
    } else {
        Measured::exact(values.iter().sum::<f64>() / values.len() as f64)
    }
}

/// The displacement parameters of occupant `index` over the orbit, each
/// brought back to the representative by its operation (`W⁻¹·U·W⁻ᵀ`) and
/// averaged: the result has the site symmetry. Only those of the same kind
/// (isotropic or not) as the representative's take part. When all agree,
/// the representative's own, with their uncertainties.
fn averaged_adp(
    contributions: &[(usize, &Observation)],
    rotations: &[[[f64; 3]; 3]],
    index: usize,
    own: Adp,
    reciprocal: &[f64; 3],
) -> Adp {
    let brought: Vec<Adp> = contributions
        .iter()
        .filter_map(|(g, o)| {
            let adp = o.occupants.get(index)?.adp?;
            let same_kind = matches!(
                (adp, own),
                (Adp::Isotropic(_), Adp::Isotropic(_)) | (Adp::Anisotropic(_), Adp::Anisotropic(_))
            );
            same_kind.then(|| transform_adp(adp, &rotations[*g], reciprocal, reciprocal))
        })
        .collect();
    let equal = |a: f64, b: f64| (a - b).abs() <= SAME * a.abs().max(b.abs()).max(1e-3);
    match own {
        Adp::Isotropic(u) => {
            let values: Vec<f64> = brought
                .iter()
                .filter_map(|adp| match adp {
                    Adp::Isotropic(v) => Some(v.value()),
                    Adp::Anisotropic(_) => None,
                })
                .collect();
            if values.iter().all(|v| equal(*v, u.value())) {
                own
            } else {
                Adp::Isotropic(Measured::exact(
                    values.iter().sum::<f64>() / values.len() as f64,
                ))
            }
        }
        Adp::Anisotropic(u) => {
            let values: Vec<[f64; 6]> = brought
                .iter()
                .filter_map(|adp| match adp {
                    Adp::Anisotropic(v) => Some(v.map(|m| m.value())),
                    Adp::Isotropic(_) => None,
                })
                .collect();
            let own_values = u.map(|m| m.value());
            if values
                .iter()
                .all(|v| (0..6).all(|i| equal(v[i], own_values[i])))
            {
                own
            } else {
                let n = values.len() as f64;
                Adp::Anisotropic(
                    [0, 1, 2, 3, 4, 5].map(|i| {
                        Measured::exact(tidy(values.iter().map(|v| v[i]).sum::<f64>() / n))
                    }),
                )
            }
        }
    }
}

/// Labels for the merged sites: each occupant keeps the label of the
/// representative's, except that labels the P1 expansion made, `O1_1`,
/// `O1_2`, …, all from one `O1`, give back `O1` when it is free.
fn merge_labels(
    sites: &mut [Site],
    orbits: &[FrameOrbit],
    clusters: &[Cluster],
    observations: &[Observation],
) {
    let mut proposals: Vec<Vec<Option<String>>> = Vec::with_capacity(sites.len());
    for (site, orbit) in sites.iter().zip(orbits) {
        let mut members: Vec<usize> = orbit.images.iter().map(|(k, _)| *k).collect();
        members.sort_unstable();
        members.dedup();
        let labels_of = |index: usize| -> Vec<&str> {
            members
                .iter()
                .flat_map(|k| &clusters[*k].observations)
                .filter_map(|o| observations[*o].occupants.get(index))
                .map(|o| o.label.as_str())
                .collect()
        };
        proposals.push(
            (0..site.occupants.len())
                .map(|index| common_base(&labels_of(index)))
                .collect(),
        );
    }
    // A base is taken when no other proposal and no kept label has it.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (site, proposed) in sites.iter().zip(&proposals) {
        for (occupant, proposal) in site.occupants.iter().zip(proposed) {
            let label = proposal.clone().unwrap_or_else(|| occupant.label.clone());
            *counts.entry(label).or_default() += 1;
        }
    }
    for (site, proposed) in sites.iter_mut().zip(proposals) {
        for (occupant, proposal) in site.occupants.iter_mut().zip(proposed) {
            if let Some(base) = proposal.filter(|base| counts.get(base) == Some(&1)) {
                occupant.label = base;
            }
        }
    }
}

/// `O1` when every label is `O1_k` for different numbers k.
fn common_base(labels: &[&str]) -> Option<String> {
    let mut base = None;
    let mut numbers = HashSet::new();
    for label in labels {
        let (stem, number) = label.rsplit_once('_')?;
        let number: u32 = number.parse().ok()?;
        if stem.is_empty() || !numbers.insert(number) {
            return None;
        }
        match base {
            None => base = Some(stem),
            Some(b) if b == stem => {}
            Some(_) => return None,
        }
    }
    base.map(str::to_string)
}

/// The position map of a change of cell: `x' = P⁻¹·(x − p)`.
fn change_map(change: &CellTransform) -> PositionMap {
    let inverse = cell_transform::mat_inverse(&change.exact_matrix())
        .map(|m| to_f64(&m))
        .unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    let origin = change.origin().map(Rational::to_f64);
    let shift = [0, 1, 2].map(|i| -(0..3).map(|j| inverse[i][j] * origin[j]).sum::<f64>());
    PositionMap::new(inverse, shift)
}

fn is_identity(mapping: &PositionMap) -> bool {
    let x = mapping.apply([0.0; 3]);
    let columns = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    columns.iter().enumerate().all(|(j, e)| {
        let y = mapping.apply(*e);
        (0..3).all(|i| {
            let expected = if i == j { 1.0 } else { 0.0 };
            (y[i] - x[i] - expected).abs() < 1e-9
        })
    })
}

/// Å from the structure's origin to the result's, up to a lattice
/// translation of the structure.
fn origin_shift(structure: &Structure, mapping: &PositionMap) -> f64 {
    let Some(back) = mapping.inverse() else {
        return 0.0;
    };
    let origin = back.apply([0.0; 3]);
    let reduced = origin.map(|v| v - v.round());
    let length = norm(&structure.lattice.params().metric(), reduced);
    if length < 1e-9 { 0.0 } else { length }
}

fn same_orientation(a: &Orientation, b: &Orientation) -> bool {
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let rows = |o: &Orientation| o.rotation().map_or(identity, |r| *r.rows());
    let (a, b) = (rows(a), rows(b));
    (0..3).all(|i| (0..3).all(|j| (a[i][j] - b[i][j]).abs() < 1e-9))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{self, SampleKey};
    use crate::{
        AtomFate, CellPreset, Command, CrystalDocument, Derived, Edit, EditError, Measurement,
        Rotation, Species,
    };

    const SYMPREC: f64 = 0.01;

    fn rutile() -> Structure {
        sample::load(SampleKey::Rutile).structure
    }

    fn srtio3() -> Structure {
        sample::load(SampleKey::StrontiumTitanate).structure
    }

    fn p1(structure: &Structure) -> Structure {
        transform::expand_to_p1(structure).structure
    }

    fn keep(structure: &Structure) -> Result<Symmetrization, SymmetrizeError> {
        DetectSymmetry::new(structure).run(structure)
    }

    fn standardize(structure: &Structure) -> Result<Symmetrization, SymmetrizeError> {
        DetectSymmetry {
            symprec: SYMPREC,
            standardize: true,
        }
        .run(structure)
    }

    fn number(result: &Symmetrization) -> u16 {
        result.space_group.as_ref().map_or(0, |sg| sg.number)
    }

    fn atoms_in_cell(structure: &Structure) -> usize {
        expand::orbits(structure)
            .iter()
            .map(|o| o.positions.len())
            .sum()
    }

    /// The atoms of the cell in Cartesian Å, with their elements.
    fn cartesian(structure: &Structure) -> Vec<(String, [f64; 3])> {
        expand::orbits(structure)
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

    /// Whether every atom of each structure lies within `tolerance` Å of an
    /// atom of the same element of the other, up to a lattice translation,
    /// with as many atoms per volume: the same crystal, in any cells.
    fn same_crystal(a: &Structure, b: &Structure, tolerance: f64) -> bool {
        let covered = |from: &Structure, onto: &Structure| {
            let atoms = cartesian(onto);
            let metric = onto.lattice.params().metric();
            cartesian(from).iter().all(|(element, r)| {
                atoms.iter().any(|(e, s)| {
                    let d = onto
                        .lattice
                        .to_fractional([0, 1, 2].map(|i| r[i] - s[i]))
                        .map(|v| v - v.round());
                    e == element && norm(&metric, d) < tolerance
                })
            })
        };
        let density = |s: &Structure| atoms_in_cell(s) as f64 / s.lattice.params().volume();
        (density(a) - density(b)).abs() < 1e-6 * density(a) && covered(a, b) && covered(b, a)
    }

    /// Moves every site of a structure by a different Cartesian vector of
    /// length up to `amount` Å.
    fn perturbed(structure: &Structure, amount: f64) -> Structure {
        let mut moved = structure.clone();
        for (k, site) in moved.sites.iter_mut().enumerate() {
            let k = k as f64 + 1.0;
            let direction = [(k * 12.9898).sin(), (k * 78.233).sin(), (k * 37.719).sin()];
            let length = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
            let scale = amount * (0.5 + 0.5 * (k * 0.618).fract()) / length;
            let d = structure
                .lattice
                .to_fractional(direction.map(|v| v * scale));
            for (x, d) in site.position.iter_mut().zip(d) {
                *x += d;
            }
        }
        moved
    }

    fn same_operations(a: &Structure, b: &Structure) -> bool {
        tables::same_operations(&a.symmetry, &b.symmetry)
    }

    fn labels(structure: &Structure) -> Vec<&str> {
        structure.sites.iter().map(Site::label).collect()
    }

    #[test]
    fn expanded_structures_come_back_as_they_were() {
        for structure in [rutile(), srtio3()] {
            let result = keep(&p1(&structure)).unwrap();
            let back = &result.structure;
            assert!(same_operations(back, &structure));
            assert_eq!(labels(back), labels(&structure));
            for (site, original) in back.sites.iter().zip(&structure.sites) {
                assert_eq!(site.position, wrap(original.position));
            }
            assert_eq!(back.lattice, structure.lattice);
            assert_eq!(result.max_displacement, 0.0);
            assert_eq!(result.origin_shift, 0.0);
            assert!(!result.cell_changed && !result.orientation_changed);
            assert_eq!(result.detected_number, number(&result));
        }
    }

    #[test]
    fn atoms_are_averaged_onto_their_symmetric_positions() {
        let rutile = rutile();
        let expanded = p1(&rutile);
        let moved = perturbed(&expanded, 0.006);
        let result = keep(&moved).unwrap();
        assert_eq!(number(&result), 136);
        assert_eq!(result.structure.sites.len(), 2);
        assert!(same_crystal(&result.structure, &rutile, 0.004));
        // Ti sits on 2a, fixed; O on 4f, x, x, 0.
        let derived = Derived::compute(&result.structure);
        let o = result.structure.sites[1].position;
        assert!((o[0] - o[1]).abs() < 1e-12 && o[2].abs() < 1e-12);
        assert_eq!(derived.multiplicity(result.structure.sites[1].id), Some(4));
        // Each atom moved by its own displacement, at most 0.006 Å, give or
        // take what the average moved.
        assert!(result.max_displacement > 0.002 && result.max_displacement < 0.009);
        // The cell parameters hold what symmetry requires.
        let mut skewed = moved.clone();
        let mut params = *skewed.lattice.params();
        params.b = Measured::with_su(4.5941, 0.0002);
        skewed.lattice.set_params(params);
        let result = keep(&skewed).unwrap();
        let params = result.structure.lattice.params();
        assert_eq!(params.a, params.b);
        assert!(
            (params.a.value() - (4.594_f64.powi(2) / 2.0 + 4.5941_f64.powi(2) / 2.0).sqrt()).abs()
                < 1e-12
        );
        assert_eq!(params.a.su(), None);
        // c did not change: it keeps its value and uncertainty.
        assert_eq!(params.c, rutile.lattice.params().c);
    }

    /// A structure of the tabulated setting `hall_number`: an atom on a
    /// general position with anisotropic displacement parameters, and one on
    /// a special position, if the setting has one, with an isotropic one.
    fn structure_of(hall_number: u16) -> Structure {
        let setting = tables::setting(hall_number);
        let symmetry = setting.symmetry.clone();
        let generic = CellParams::new(10.2, 12.4, 14.6, 81.0, 86.0, 97.0);
        let params =
            CellParams::from_metric(constraints::symmetrized_metric(&symmetry, generic.metric()));
        let other = Site::new(
            SiteId(3),
            [0.4127, 0.0813, 0.2941],
            vec![Occupant::new(
                "Ge1",
                Species::element("Ge"),
                Measured::exact(1.0),
            )],
        );
        let general = Site::new(
            SiteId(1),
            [0.0713, 0.3291, 0.1847],
            vec![
                Occupant::new("Si1", Species::element("Si"), Measured::exact(1.0)).with_adp(
                    Adp::Anisotropic([
                        Measured::with_su(0.010, 0.001),
                        Measured::with_su(0.012, 0.001),
                        Measured::with_su(0.015, 0.002),
                        Measured::exact(0.001),
                        Measured::exact(0.002),
                        Measured::exact(0.003),
                    ]),
                ),
            ],
        )
        .with_position_su([Some(0.0002), Some(0.0003), None]);
        // Two orbits on general positions, of different elements: one alone
        // could have more symmetry than the setting, e.g. an inversion
        // center between its two atoms in P2₁.
        let mut sites = vec![general, other];
        let entries: Vec<_> = tables::wyckoff_positions(hall_number).collect();
        let most = entries.iter().map(|w| w.multiplicity).max().unwrap_or(0);
        if let Some(entry) = entries.iter().find(|w| w.multiplicity < most) {
            let position = entry.coordinates.apply([0.2137, 0.4419, 0.3683]);
            sites.push(Site::new(
                SiteId(2),
                position,
                vec![
                    Occupant::new("O1", Species::element("O"), Measured::exact(1.0))
                        .with_adp(Adp::Isotropic(Measured::with_su(0.02, 0.01))),
                ],
            ));
        }
        Structure::new(Lattice::new(params), symmetry, sites)
    }

    /// The v0.2 acceptance test (roadmap §6): every tabulated setting,
    /// expanded to P1 and perturbed by less than the tolerance, symmetrizes
    /// back to its operations, sites and labels, and expands to the same
    /// atoms within the tolerance. Some lattices are turned, some
    /// left-handed; the displacement parameters come back exactly.
    #[test]
    fn every_tabulated_setting_survives_p1_and_back() {
        let turn = Rotation::new([[0.0, -0.6, 0.8], [1.0, 0.0, 0.0], [0.0, 0.8, 0.6]]).unwrap();
        let mirror = Rotation::new([[0.6, 0.8, 0.0], [0.8, -0.6, 0.0], [0.0, 0.0, 1.0]]).unwrap();
        for hall_number in 2..=530 {
            let mut structure = structure_of(hall_number);
            let label = format!("Hall {hall_number}");
            if hall_number % 5 == 0 {
                structure.lattice =
                    Lattice::with_orientation(*structure.lattice.params(), Orientation::new(turn));
            } else if hall_number % 7 == 0 {
                structure.lattice = Lattice::with_orientation(
                    *structure.lattice.params(),
                    Orientation::new(mirror),
                );
            }
            let expanded = p1(&structure);
            let moved = perturbed(&expanded, 0.4 * SYMPREC);
            let result = keep(&moved).unwrap_or_else(|e| panic!("{label}: {e}"));
            let back = &result.structure;
            let expected = SpaceGroupType::identify(
                &transform::right_handed(&structure).symmetry,
                structure.lattice.params(),
            )
            .unwrap();
            assert_eq!(number(&result), expected.number, "{label}");
            assert!(same_operations(back, &structure), "{label}");
            assert_eq!(labels(back), labels(&structure), "{label}");
            assert_eq!(
                back.lattice.is_left_handed(),
                structure.lattice.is_left_handed(),
                "{label}"
            );
            assert!(
                result.max_displacement < SYMPREC,
                "{label}: {}",
                result.max_displacement
            );
            assert_eq!(result.origin_shift, 0.0, "{label}");
            assert!(same_crystal(&p1(back), &expanded, SYMPREC), "{label}");
            for (site, original) in back.sites.iter().zip(&structure.sites) {
                assert_eq!(site.occupants[0].adp, original.occupants[0].adp, "{label}");
            }
        }
    }

    #[test]
    fn standardizing_gives_the_conventional_cell() {
        // fcc copper in its primitive cell, written out in P1.
        let conventional = Structure::new(
            Lattice::new(CellParams::new(3.615, 3.615, 3.615, 90.0, 90.0, 90.0)),
            Symmetry::from_hm_symbol("F m -3 m").unwrap(),
            vec![Site::single(1, "Cu1", "Cu", [0.0; 3])],
        );
        let primitive = CellPreset::Primitive.transform(&conventional).unwrap();
        let primitive = Edit::TransformCell {
            transform: primitive,
        }
        .apply(&conventional)
        .unwrap()
        .structure;
        let input = p1(&primitive);
        assert_eq!(atoms_in_cell(&input), 1);
        // Kept, the primitive cell holds Fm-3m in a setting of its own.
        let kept = keep(&input).unwrap();
        assert_eq!(kept.structure.symmetry.operations().len(), 48);
        assert!(!kept.cell_changed);
        // Standardized: the conventional cell in the standard orientation.
        let result = standardize(&input).unwrap();
        let s = &result.structure;
        assert_eq!(number(&result), 225);
        assert_eq!(s.symmetry.operations().len(), 192);
        assert_eq!(atoms_in_cell(s), 4);
        assert_eq!(s.lattice, conventional.lattice);
        assert_eq!(s.sites[0].position, [0.0; 3]);
        assert!(result.cell_changed && result.orientation_changed);
        assert!(same_crystal(s, &input, 1e-9));

        // A structure in its conventional cell already keeps it.
        let srtio3 = srtio3();
        let result = standardize(&p1(&srtio3)).unwrap();
        assert!(!result.cell_changed && !result.orientation_changed);
        assert!(same_operations(&result.structure, &srtio3));
        assert_eq!(labels(&result.structure), labels(&srtio3));
    }

    #[test]
    fn supercells_symmetrize_to_their_own_cell() {
        let rutile = rutile();
        let supercell = sample::load(SampleKey::RutileSupercell).structure;
        assert_eq!(supercell.sites.len(), 48);
        // Kept: the supercell's translations become centering.
        let kept = keep(&supercell).unwrap();
        assert_eq!(kept.structure.symmetry.operations().len(), 16 * 8);
        assert_eq!(kept.structure.sites.len(), 2);
        assert!(same_crystal(&kept.structure, &supercell, 1e-9));
        // Standardized: the rutile cell, with a cell an eighth the size.
        let result = standardize(&supercell).unwrap();
        assert!(same_operations(&result.structure, &rutile));
        assert_eq!(atoms_in_cell(&result.structure), 6);
        assert_eq!(result.structure.lattice, rutile.lattice);
        assert!(same_crystal(&result.structure, &rutile, 1e-9));

        // A 2a × a × c cell cannot hold the fourfold axis: kept, it gives
        // an orthorhombic subgroup; standardized, all of P4₂/mnm.
        let long = crate::make_supercell(&rutile, [[2, 0, 0], [0, 1, 0], [0, 0, 1]]).unwrap();
        let kept = keep(&long).unwrap();
        assert_eq!(kept.detected_number, 136);
        let subgroup = number(&kept);
        assert!((16..=74).contains(&subgroup), "{subgroup}");
        assert!(same_crystal(&kept.structure, &long, 1e-9));
        let result = standardize(&long).unwrap();
        assert_eq!(number(&result), 136);
        assert!(same_crystal(&result.structure, &rutile, 1e-9));

        // A cell too large to keep: 64 cells of SrTiO3 would need 3072
        // operations; standardizing still works.
        let srtio3 = srtio3();
        let big = crate::make_supercell(&srtio3, [[4, 0, 0], [0, 4, 0], [0, 0, 4]]).unwrap();
        assert_eq!(keep(&big).unwrap_err(), SymmetrizeError::TooLarge);
        let result = standardize(&big).unwrap();
        assert!(same_operations(&result.structure, &srtio3));
        assert_eq!(result.structure.lattice, srtio3.lattice);
    }

    #[test]
    fn the_origin_stays_where_the_symmetry_allows() {
        let srtio3 = srtio3();
        let expanded = p1(&srtio3);
        let shift = |structure: &Structure, by: [f64; 3]| {
            let mut moved = structure.clone();
            for site in &mut moved.sites {
                for (x, d) in site.position.iter_mut().zip(by) {
                    *x += d;
                }
            }
            moved
        };
        // Within the tolerance of an origin of Pm-3m, the origin stays and
        // the atoms move.
        let a = srtio3.lattice.params().a.value();
        let result = keep(&shift(&expanded, [0.003 / a, 0.0, 0.0])).unwrap();
        assert_eq!(result.origin_shift, 0.0);
        assert!((result.max_displacement - 0.003).abs() < 1e-9);
        assert!(same_crystal(&result.structure, &srtio3, 1e-9));
        // Further away, the origin moves to the nearest one: the atoms do
        // not move, and the operations are the tabulated ones.
        let by = [0.1234, 0.0567, 0.0891];
        let result = keep(&shift(&expanded, by)).unwrap();
        assert!(same_operations(&result.structure, &srtio3));
        let expected = norm(&srtio3.lattice.params().metric(), by);
        assert!((result.origin_shift - expected).abs() < 1e-9);
        assert!(result.max_displacement < 1e-9);
        assert!(same_crystal(&result.structure, &srtio3, 1e-9));

        // Along a polar axis any origin will do: ZnO moved along c keeps
        // its origin, with the tabulated operations of P6₃mc.
        let zno = Structure::new(
            Lattice::new(CellParams::new(3.25, 3.25, 5.21, 90.0, 90.0, 120.0)),
            Symmetry::from_hm_symbol("P 63 m c").unwrap(),
            vec![
                Site::single(1, "Zn1", "Zn", [1.0 / 3.0, 2.0 / 3.0, 0.0]),
                Site::single(2, "O1", "O", [1.0 / 3.0, 2.0 / 3.0, 0.382]),
            ],
        );
        let result = keep(&shift(&p1(&zno), [0.0, 0.0, 0.1])).unwrap();
        assert_eq!(result.origin_shift, 0.0);
        assert!(same_operations(&result.structure, &zno));
        assert!((result.structure.sites[0].position[2] - 0.1).abs() < 1e-12);
    }

    #[test]
    fn occupancies_and_displacement_parameters_are_averaged() {
        let mut rutile = rutile();
        let u = [0.010, 0.010, 0.012, 0.002, 0.0, 0.0].map(|v| Measured::with_su(v, 0.001));
        rutile.sites[1].occupants[0].adp = Some(Adp::Anisotropic(u));
        rutile.sites[0].occupants[0].adp = Some(Adp::Isotropic(Measured::with_su(0.006, 0.001)));
        let mut expanded = p1(&rutile);
        // The four oxygens differ a little; they stay one kind.
        for (k, site) in expanded
            .sites
            .iter_mut()
            .filter(|s| s.element() == "O")
            .enumerate()
        {
            let occupant = &mut site.occupants[0];
            occupant.occupancy = Measured::exact(0.98 + 0.0001 * k as f64);
            if let Some(Adp::Anisotropic(u)) = &mut occupant.adp {
                u[2] = Measured::exact(0.012 + 0.0004 * k as f64);
            }
        }
        let result = keep(&expanded).unwrap();
        let o = &result.structure.sites[1].occupants[0];
        assert!((o.occupancy.value() - 0.98015).abs() < 1e-12);
        assert_eq!(o.occupancy.su(), None);
        let Some(Adp::Anisotropic(u)) = o.adp else {
            panic!("anisotropic");
        };
        assert!((u[2].value() - 0.0126).abs() < 1e-12);
        assert_eq!(u[2].su(), None);
        // The average has the site's symmetry: U11 = U22, U13 = U23 = 0.
        assert_eq!(u[0].value(), u[1].value());
        assert!(u[4].value().abs() < 1e-15 && u[5].value().abs() < 1e-15);
        // Ti's atoms all agree: their values and uncertainties stay.
        let ti = &result.structure.sites[0].occupants[0];
        assert_eq!(ti.adp, rutile.sites[0].occupants[0].adp);
        assert_eq!(ti.occupancy, rutile.sites[0].occupants[0].occupancy);
    }

    #[test]
    fn kinds_keep_atoms_apart() {
        // One oxygen half occupied: no longer equivalent to the others.
        let mut expanded = p1(&srtio3());
        let o = expanded
            .sites
            .iter_mut()
            .find(|s| s.element() == "O")
            .unwrap();
        o.occupants[0].occupancy = Measured::exact(0.5);
        let result = keep(&expanded).unwrap();
        assert_eq!(number(&result), 123);
        assert_eq!(result.structure.sites.len(), 4);
        // Nor are atoms of different disorder groups.
        let mut expanded = p1(&srtio3());
        let o = expanded
            .sites
            .iter_mut()
            .find(|s| s.element() == "O")
            .unwrap();
        o.disorder = Some(crate::Disorder::new(None, "2".into()));
        assert_eq!(number(&keep(&expanded).unwrap()), 123);
    }

    #[test]
    fn what_cannot_be_symmetrized() {
        let expanded = p1(&rutile());
        for symprec in [0.0, -0.01, f64::NAN, f64::INFINITY] {
            let detect = DetectSymmetry {
                symprec,
                standardize: false,
            };
            assert_eq!(
                detect.run(&expanded).unwrap_err(),
                SymmetrizeError::InvalidTolerance
            );
        }
        let mut empty = expanded.clone();
        empty.sites.clear();
        assert_eq!(keep(&empty).unwrap_err(), SymmetrizeError::NoAtoms);
        // Nothing beyond P1.
        let triclinic = Structure::new(
            Lattice::new(CellParams::new(5.1, 6.2, 7.3, 81.0, 86.0, 97.0)),
            Symmetry::p1(),
            vec![
                Site::single(1, "Si1", "Si", [0.11, 0.23, 0.37]),
                Site::single(2, "O1", "O", [0.41, 0.53, 0.29]),
                Site::single(3, "O2", "O", [0.71, 0.13, 0.83]),
            ],
        );
        assert_eq!(
            keep(&triclinic).unwrap_err(),
            SymmetrizeError::NoHigherSymmetry
        );
        // Nothing beyond what the structure already has.
        assert_eq!(
            keep(&rutile()).unwrap_err(),
            SymmetrizeError::NoHigherSymmetry
        );
        // Two atoms of a kind on one spot.
        let mut doubled = expanded.clone();
        let mut copy = doubled.sites[0].clone();
        copy.id = SiteId(99);
        copy.occupants[0].label = "Ti9".into();
        doubled.sites.push(copy);
        assert_eq!(keep(&doubled).unwrap_err(), SymmetrizeError::NotFound);
        // An inversion center 0.003 Å from an atom: a tolerance below the
        // position tolerance keeps the atom and its image apart, 0.006 Å
        // apart, which the position tolerance would merge.
        let cell = CellParams::new(5.1, 6.2, 7.3, 81.0, 86.0, 97.0);
        let d = Lattice::new(cell).to_fractional([0.003, 0.0, 0.0]);
        let centered = Structure::new(
            Lattice::new(cell),
            Symmetry::p1(),
            vec![
                Site::single(1, "Si1", "Si", d),
                Site::single(2, "Si2", "Si", d.map(|v| -v)),
                Site::single(3, "O1", "O", [0.21, 0.33, 0.17]),
                Site::single(4, "O2", "O", [-0.21, -0.33, -0.17]),
            ],
        );
        let fine = DetectSymmetry {
            symprec: 0.001,
            standardize: false,
        };
        assert_eq!(
            fine.run(&centered).unwrap_err(),
            SymmetrizeError::PositionTolerance
        );
        // With the default tolerance, the two atoms overlap.
        assert_eq!(keep(&centered).unwrap_err(), SymmetrizeError::NotFound);
    }

    #[test]
    fn references_follow_the_atoms_and_undo_restores_them() {
        let expanded = p1(&rutile());
        let mut document = CrystalDocument::new(expanded.clone());
        let derived = document.derived().clone();
        let o = derived
            .atoms
            .iter()
            .find(|a| expanded.site(a.atom.site).unwrap().element() == "O")
            .unwrap()
            .atom;
        let ti = derived.atoms[0].atom;
        let measurement = Measurement::Distance([ti, o]);
        document
            .execute(Command::AddMeasurement(measurement))
            .unwrap();
        let length = crate::distance(
            derived.cartesian(&expanded.lattice, ti).unwrap(),
            derived.cartesian(&expanded.lattice, o).unwrap(),
        );
        let outcome = document
            .edit(Edit::Symmetrize {
                symprec: None,
                standardize: false,
            })
            .unwrap();
        assert!(outcome.changed);
        let symmetrized = document.structure().clone();
        assert_eq!(symmetrized.sites.len(), 2);
        // Every atom is the atom at the same place.
        for atom in &derived.atoms {
            let AtomFate::Kept(kept) = outcome.ids.atom(atom.atom) else {
                panic!("{:?} lost", atom.atom);
            };
            let before = derived.fractional(atom.atom).unwrap();
            let after = document.derived().fractional(kept).unwrap();
            assert!(
                (0..3).all(|i| (before[i] - after[i]).abs() < 1e-12),
                "{atom:?}"
            );
        }
        let annotation = document.annotations()[0];
        assert!(!annotation.stale);
        let [a, b] = annotation.measurement.atoms()[..] else {
            panic!("two atoms");
        };
        let after = crate::distance(
            document
                .derived()
                .cartesian(&symmetrized.lattice, a)
                .unwrap(),
            document
                .derived()
                .cartesian(&symmetrized.lattice, b)
                .unwrap(),
        );
        assert!((after - length).abs() < 1e-12);
        // Undo puts back the P1 structure exactly, and the references.
        let undone = document.undo().unwrap();
        assert_eq!(document.structure(), &expanded);
        let kept = |atom| match outcome.ids.atom(atom) {
            AtomFate::Kept(kept) => kept,
            _ => unreachable!(),
        };
        assert_eq!(undone.ids.atom(kept(o)), AtomFate::Kept(o));
        assert_eq!(undone.ids.atom(kept(ti)), AtomFate::Kept(ti));
        assert_eq!(document.annotations()[0].measurement, measurement);
        // Redo puts back the symmetrized structure exactly.
        document.redo().unwrap();
        assert_eq!(document.structure(), &symmetrized);

        // Through a change of cell too: the supercell's atoms are the
        // atoms of the conventional cell at the same place.
        let supercell = sample::load(SampleKey::RutileSupercell).structure;
        let edit = Edit::Symmetrize {
            symprec: None,
            standardize: true,
        };
        let applied = edit.apply(&supercell).unwrap();
        let before = Derived::compute(&supercell);
        let after = Derived::compute(&applied.structure);
        for atom in &before.atoms {
            let AtomFate::Kept(kept) = applied.ids.atom(atom.atom) else {
                panic!("{:?} lost", atom.atom);
            };
            let r = supercell
                .lattice
                .to_cartesian(before.fractional(atom.atom).unwrap());
            let s = applied
                .structure
                .lattice
                .to_cartesian(after.fractional(kept).unwrap());
            assert!((0..3).all(|i| (r[i] - s[i]).abs() < 1e-9), "{atom:?}");
        }
        let restored = applied.inverse.apply(&applied.structure).unwrap();
        assert_eq!(restored.structure, supercell);
        assert_eq!(restored.ids.sites(applied.structure.sites[0].id).len(), 16);
    }

    #[test]
    fn left_handed_lattices_keep_their_hand() {
        // Atoms of P3₁21 written out in P1 on a left-handed basis: the
        // mirror image, P3₂21.
        let hexagonal = CellParams::new(4.9, 4.9, 5.4, 90.0, 90.0, 120.0);
        let quartz = Structure::new(
            Lattice::new(hexagonal),
            Symmetry::from_hm_symbol("P 31 2 1").unwrap(),
            vec![
                Site::single(1, "Si1", "Si", [0.47, 0.0, 1.0 / 3.0]),
                Site::single(2, "O1", "O", [0.41, 0.27, 0.12]),
            ],
        );
        let mirror = Rotation::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]]).unwrap();
        let mut expanded = p1(&quartz);
        expanded.lattice = Lattice::with_orientation(hexagonal, Orientation::new(mirror));
        let result = keep(&expanded).unwrap();
        assert_eq!(number(&result), 154);
        assert!(result.structure.lattice.is_left_handed());
        assert!(same_operations(&result.structure, &quartz));
        assert!(same_crystal(&result.structure, &expanded, 1e-9));
        assert!(!result.orientation_changed);
        // Standardized, it is described on a right-handed basis.
        let result = standardize(&expanded).unwrap();
        assert_eq!(number(&result), 154);
        assert!(!result.structure.lattice.is_left_handed());
        assert!(result.cell_changed && result.orientation_changed);
        assert!(same_crystal(&result.structure, &expanded, 1e-9));
    }

    #[test]
    fn edits_serialize_as_tagged_json() {
        let edit = Edit::Symmetrize {
            symprec: None,
            standardize: true,
        };
        let json = serde_json::to_value(&edit).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"command": "symmetrize", "standardize": true})
        );
        let read: Edit =
            serde_json::from_str(r#"{"command": "symmetrize", "symprec": 0.05}"#).unwrap();
        assert_eq!(
            read,
            Edit::Symmetrize {
                symprec: Some(0.05),
                standardize: false
            }
        );
        let applied = edit.apply(&p1(&rutile())).unwrap();
        let json = serde_json::to_string(&applied.inverse).unwrap();
        assert_eq!(
            serde_json::from_str::<Edit>(&json).unwrap(),
            applied.inverse
        );
        // Refused edits say why.
        assert_eq!(
            Edit::Symmetrize {
                symprec: Some(0.0),
                standardize: false
            }
            .apply(&rutile())
            .unwrap_err(),
            EditError::Symmetrize(SymmetrizeError::InvalidTolerance)
        );
    }

    #[test]
    fn labels_of_the_p1_expansion_merge_back() {
        assert_eq!(common_base(&["O1_1", "O1_2", "O1_4"]), Some("O1".into()));
        assert_eq!(common_base(&["O1_1", "O2_2"]), None);
        assert_eq!(common_base(&["O1", "O2"]), None);
        assert_eq!(common_base(&["O1_1", "O1_1"]), None);
        assert_eq!(common_base(&["_1"]), None);
        assert_eq!(common_base(&["Si_a_1", "Si_a_2"]), Some("Si_a".into()));
    }
}
