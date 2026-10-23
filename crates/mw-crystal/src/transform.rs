//! Describing a structure in another cell (design §7.1, §7.8, §7.9): the P1
//! expansion, where every equivalent atom becomes a site of its own,
//! changes of cell, and supercells. Displacement parameters and coordinate
//! uncertainties follow the atoms through the operations and the change of
//! basis.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::fmt;

use nalgebra::Matrix3;

use crate::cell_transform::{self, Exact};
use crate::id_map::Rebasing;
use crate::{
    Adp, CellParam, CellParams, CellTransform, IdMap, Lattice, Measured, Occupant, Orientation,
    Rotation, Site, SiteFate, SiteId, SpaceGroup, Structure, SymOp, Symmetry, TransformError,
    expand, tables,
};

/// Coefficients this close to an integer are that integer, so that values
/// that only move between components (a = b under a fourfold axis) come out
/// exactly as they were.
const SNAP: f64 = 1e-9;

fn snap(value: f64) -> f64 {
    let rounded = value.round();
    if (value - rounded).abs() < SNAP {
        rounded
    } else {
        value
    }
}

/// The P1 expansion of a structure and what became of each of its sites.
pub(crate) struct Expanded {
    pub structure: Structure,
    /// For each site, the new sites its orbit members became, in member
    /// order.
    pub splits: Vec<(SiteId, Vec<SiteId>)>,
}

/// One atom of the P1 expansion before it is labeled: the site it came
/// from, its member number, its position in [0, 1) and its occupants with
/// their displacement parameters carried through the operation.
struct P1Atom<'a> {
    site: &'a Site,
    member: usize,
    position: [f64; 3],
    position_su: [Option<f64>; 3],
    occupants: Vec<Occupant>,
}

fn p1_atoms(structure: &Structure) -> Vec<P1Atom<'_>> {
    let reciprocal = structure.lattice.params().reciprocal_lengths();
    let operations = structure.symmetry.operations();
    let mut atoms = Vec::new();
    for site in &structure.sites {
        for (member, (op, position)) in expand::members(structure, site).into_iter().enumerate() {
            let rotation = operations[op].rotation().map(|row| row.map(f64::from));
            let occupants = site
                .occupants
                .iter()
                .map(|occupant| Occupant {
                    adp: occupant
                        .adp
                        .map(|adp| transform_adp(adp, &rotation, &reciprocal, &reciprocal)),
                    ..occupant.clone()
                })
                .collect();
            atoms.push(P1Atom {
                site,
                member,
                position,
                position_su: transform_su(site.position_su, &rotation),
                occupants,
            });
        }
    }
    atoms
}

/// Hands out labels unique among those handed out.
#[derive(Default)]
struct Labels(HashSet<String>);

impl Labels {
    fn unique(&mut self, base: String) -> String {
        let label = if self.0.contains(&base) {
            (2..)
                .map(|n| format!("{base}_{n}"))
                .find(|label| !self.0.contains(label))
                .expect("some label is free")
        } else {
            base
        };
        self.0.insert(label.clone());
        label
    }
}

fn first_free_id(structure: &Structure) -> u32 {
    structure.sites.iter().map(|s| s.id.0).max().unwrap_or(0) + 1
}

fn p1_space_group() -> SpaceGroup {
    SpaceGroup::new(Some("P 1".into()), Some(1))
}

/// Expands a structure to P1 (design §7.1): every orbit member becomes a
/// site at its position in [0, 1), with new site ids. Member `k` (counted
/// from 1) of an orbit of several atoms is labeled `label_k`; a site with
/// one atom in the cell keeps its label.
pub(crate) fn expand_to_p1(structure: &Structure) -> Expanded {
    let atoms = p1_atoms(structure);
    let several: BTreeMap<SiteId, bool> = structure
        .sites
        .iter()
        .map(|site| {
            let members = atoms.iter().filter(|a| a.site.id == site.id).count();
            (site.id, members > 1)
        })
        .collect();
    let mut labels = Labels::default();
    let mut splits: Vec<(SiteId, Vec<SiteId>)> = Vec::new();
    let mut sites = Vec::with_capacity(atoms.len());
    for (id, atom) in (first_free_id(structure)..).map(SiteId).zip(atoms) {
        let suffix = several[&atom.site.id].then(|| atom.member + 1);
        let occupants = atom
            .occupants
            .into_iter()
            .map(|occupant| Occupant {
                label: labels.unique(match suffix {
                    Some(k) => format!("{}_{k}", occupant.label),
                    None => occupant.label.clone(),
                }),
                ..occupant
            })
            .collect();
        sites.push(Site {
            id,
            position: atom.position,
            position_su: atom.position_su,
            occupants,
            disorder: atom.site.disorder.clone(),
        });
        match splits.last_mut() {
            Some((site, ids)) if *site == atom.site.id => ids.push(id),
            _ => splits.push((atom.site.id, vec![id])),
        }
    }
    let expanded = Structure {
        lattice: structure.lattice,
        symmetry: Symmetry::p1(),
        space_group: p1_space_group(),
        sites,
        tolerances: structure.tolerances,
    };
    Expanded {
        structure: expanded,
        splits,
    }
}

/// Why a supercell cannot be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupercellError {
    /// The matrix's determinant is zero or negative: it describes no cell,
    /// or a left-handed one.
    NotPositive { determinant: i64 },
}

impl fmt::Display for SupercellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPositive { determinant } => write!(
                f,
                "the supercell matrix has determinant {determinant}; it must be positive"
            ),
        }
    }
}

impl std::error::Error for SupercellError {}

fn determinant(m: &[[i64; 3]; 3]) -> i64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

/// The adjugate: `M⁻¹ = adj(M) / det(M)`.
fn adjugate(m: &[[i64; 3]; 3]) -> [[i64; 3]; 3] {
    let c = |i: usize, j: usize| {
        let (r0, r1) = ((i + 1) % 3, (i + 2) % 3);
        let (c0, c1) = ((j + 1) % 3, (j + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    // Cofactor of (j, i), transposed.
    [0, 1, 2].map(|i| [0, 1, 2].map(|j| c(j, i)))
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

/// Makes the supercell with lattice `A·M` (design §7.9; `matrix[i][j]` is
/// M's entry in row i, column j, so column j gives the new basis vector j
/// in the old basis). The result is in P1: every atom of the P1 expansion
/// has det M images, labeled `label_n` with n counting the images of that
/// occupant, cell by cell. Occupancies stay as they are.
pub fn make_supercell(
    structure: &Structure,
    matrix: [[i32; 3]; 3],
) -> Result<Structure, SupercellError> {
    let m = matrix.map(|row| row.map(i64::from));
    let det = determinant(&m);
    if det <= 0 {
        return Err(SupercellError::NotPositive { determinant: det });
    }
    // x' = adj(M)·(x + n) / det, each row reduced by its common factor so
    // that a diagonal matrix divides by its own entry and nothing else.
    let adjugate = adjugate(&m);
    let rows: [([f64; 3], f64); 3] = [0, 1, 2].map(|i| {
        let g = adjugate[i].iter().fold(det, |g, &v| gcd(g, v));
        (adjugate[i].map(|v| (v / g) as f64), (det / g) as f64)
    });
    let to_new = |x: [f64; 3]| {
        rows.map(|(coefficients, divisor)| {
            coefficients
                .iter()
                .zip(x)
                .filter(|(c, _)| **c != 0.0)
                .map(|(c, v)| c * v)
                .sum::<f64>()
                / divisor
        })
    };

    // The lattice translations whose images can fall into the new cell.
    let mut low = [i64::MAX; 3];
    let mut high = [i64::MIN; 3];
    for corner in 0..8 {
        let c = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1].map(i64::from);
        for i in 0..3 {
            let v: i64 = (0..3).map(|j| m[i][j] * c[j]).sum();
            low[i] = low[i].min(v);
            high[i] = high[i].max(v);
        }
    }

    let lattice = supercell_lattice(&structure.lattice, &m);
    let atoms = p1_atoms(structure);
    let old_reciprocal = structure.lattice.params().reciprocal_lengths();
    let new_reciprocal = lattice.params().reciprocal_lengths();
    let inverse = rows.map(|(coefficients, divisor)| coefficients.map(|c| c / divisor));
    let mut labels = Labels::default();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut next = 1;
    let mut sites = Vec::new();
    // An image in [-ε, 1 - ε) on every axis: exactly one image of each atom
    // per new cell, whatever the rounding at the faces.
    const EDGE: f64 = 1e-9;
    for i in low[0] - 1..=high[0] + 1 {
        for j in low[1] - 1..=high[1] + 1 {
            for k in low[2] - 1..=high[2] + 1 {
                let n = [i, j, k].map(|v| v as f64);
                for atom in &atoms {
                    let shifted = [0, 1, 2].map(|axis| atom.position[axis] + n[axis]);
                    let x = to_new(shifted);
                    if !x.iter().all(|v| (-EDGE..1.0 - EDGE).contains(v)) {
                        continue;
                    }
                    let position = x.map(|v| if v < 0.0 { 0.0 } else { v });
                    let occupants = atom
                        .occupants
                        .iter()
                        .map(|occupant| {
                            let count = counts.entry(occupant.label.clone()).or_default();
                            *count += 1;
                            Occupant {
                                label: labels.unique(format!("{}_{count}", occupant.label)),
                                adp: occupant.adp.map(|adp| {
                                    transform_adp(adp, &inverse, &old_reciprocal, &new_reciprocal)
                                }),
                                ..occupant.clone()
                            }
                        })
                        .collect();
                    sites.push(Site {
                        id: SiteId(next),
                        position,
                        position_su: transform_su(atom.position_su, &inverse),
                        occupants,
                        disorder: atom.site.disorder.clone(),
                    });
                    next += 1;
                }
            }
        }
    }
    debug_assert_eq!(sites.len(), atoms.len() * det as usize);
    Ok(Structure {
        lattice,
        symmetry: Symmetry::p1(),
        space_group: p1_space_group(),
        sites,
        tolerances: structure.tolerances,
    })
}

/// The same crystal on a right-handed basis, for formats that describe a
/// cell by its parameters and so imply one (CIF). A left-handed lattice
/// (design §4.2) is described by −a, −b, −c instead: the cell parameters,
/// displacement parameters and uncertainties stay as they are, coordinates
/// become −x (moved into [0, 1) when they were in it) and operations
/// (W, w) become (W, −w). Every atom stays where it was in Cartesian space.
pub fn right_handed(structure: &Structure) -> Cow<'_, Structure> {
    if structure.lattice.is_left_handed() {
        Cow::Owned(inverted(structure))
    } else {
        Cow::Borrowed(structure)
    }
}

/// The same crystal on the basis −a, −b, −c, as [`right_handed`] describes
/// it: a left-handed lattice becomes right-handed and a right-handed one
/// left-handed. Applied twice, it gives back the lattice and operations;
/// coordinates come back up to rounding and lattice translations.
pub(crate) fn inverted(structure: &Structure) -> Structure {
    let rows = structure
        .lattice
        .orientation()
        .rotation()
        .map_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], |r| {
            *r.rows()
        });
    let turned = Rotation::new(rows.map(|row| row.map(|v| -v)))
        .expect("the negative of a rotation is a rotation");
    let mut inverted = structure.clone();
    inverted.lattice =
        Lattice::with_orientation(*structure.lattice.params(), Orientation::new(turned));
    inverted.symmetry = Symmetry::from_group(
        structure
            .symmetry
            .operations()
            .iter()
            .map(|op| SymOp::new(*op.rotation(), op.translation().map(|t| -t)))
            .collect(),
    );
    for site in &mut inverted.sites {
        site.position = site.position.map(|x| {
            let v = -x;
            // `+ 0.0` turns −0 into 0.
            if v < 0.0 { v + 1.0 } else { v + 0.0 }
        });
    }
    inverted
}

/// The largest operation set a change of cell may make: Fm-3m's in a cell
/// [`cell_transform::MAX_VOLUME_FACTOR`] times its conventional one.
const MAX_OPERATIONS: usize = 192 * cell_transform::MAX_VOLUME_FACTOR as usize;

/// The operations in the new cell (design §7.8): `W' = P⁻¹·W·P`, `w' =
/// P⁻¹·(w + W·p − p)`, with the translations of the old lattice that are
/// not in the new one added as centering, and operations that differ only
/// by a translation of the new lattice merged into one.
///
/// Refused when a new basis vector is not a lattice translation of the
/// structure (its translation is not an operation), or when an operation
/// does not map the new lattice onto itself (`W'` is not an integer
/// matrix).
pub(crate) fn transform_operations(
    symmetry: &Symmetry,
    transform: &CellTransform,
) -> Result<Symmetry, TransformError> {
    let too_large = || TransformError::TooLarge;
    let p = transform.exact_matrix();
    let origin = transform.exact_origin();
    let inverse = cell_transform::mat_inverse(&p).ok_or_else(too_large)?;
    let rational = |v: Exact| v.to_rational().ok_or_else(too_large);

    // Every new basis vector must be a translation of the crystal.
    let columns = [0, 1, 2].map(|j| [0, 1, 2].map(|i| p[i][j].fract()));
    for (vector, t) in columns.iter().enumerate() {
        if t.iter().all(|v| v.is_zero()) {
            continue;
        }
        let translation = SymOp::new(
            *SymOp::IDENTITY.rotation(),
            [rational(t[0])?, rational(t[1])?, rational(t[2])?],
        );
        if !symmetry.operations().contains(&translation) {
            return Err(TransformError::NotATranslation { vector });
        }
    }

    let mut transformed = Vec::with_capacity(symmetry.operations().len());
    for op in symmetry.operations() {
        let w = op
            .rotation()
            .map(|row| row.map(|v| Exact::integer(v.into())));
        let rotated = cell_transform::mat_mul(&inverse, &w).ok_or_else(too_large)?;
        let rotation = cell_transform::mat_mul(&rotated, &p).ok_or_else(too_large)?;
        let mut integer = [[0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                let v = rotation[i][j];
                if !v.is_integer() {
                    return Err(TransformError::BreaksSymmetry { operation: *op });
                }
                integer[i][j] = i32::try_from(rational(v)?.numerator()).map_err(|_| too_large())?;
            }
        }
        // w + W·p − p, then P⁻¹.
        let wp = cell_transform::mat_apply(&w, &origin).ok_or_else(too_large)?;
        let mut shift = [Exact::ZERO; 3];
        for i in 0..3 {
            shift[i] = Exact::of(op.translation()[i])
                .add(wp[i])
                .and_then(|v| v.sub(origin[i]))
                .ok_or_else(too_large)?;
        }
        let translation = cell_transform::mat_apply(&inverse, &shift).ok_or_else(too_large)?;
        transformed.push((integer, translation));
    }

    // The old lattice in the new basis, modulo the new lattice: the
    // translations a larger cell adds as centering.
    let generators: Vec<[Exact; 3]> = (0..3)
        .map(|j| [0, 1, 2].map(|i| inverse[i][j].fract()))
        .collect();
    let mut centering = vec![[Exact::ZERO; 3]];
    let mut next = 0;
    while next < centering.len() {
        let t = centering[next];
        for g in &generators {
            let mut sum = [Exact::ZERO; 3];
            for i in 0..3 {
                sum[i] = t[i].add(g[i]).ok_or_else(too_large)?.fract();
            }
            if !centering.contains(&sum) {
                if centering.len() * transformed.len() >= MAX_OPERATIONS {
                    return Err(TransformError::Volume {
                        determinant: transform.determinant(),
                    });
                }
                centering.push(sum);
            }
        }
        next += 1;
    }

    let mut seen = HashSet::new();
    let mut operations = Vec::new();
    for t in &centering {
        for (rotation, translation) in &transformed {
            let mut shifted = [Exact::ZERO; 3];
            for i in 0..3 {
                shifted[i] = translation[i].add(t[i]).ok_or_else(too_large)?;
            }
            let op = SymOp::new(
                *rotation,
                [
                    rational(shifted[0])?,
                    rational(shifted[1])?,
                    rational(shifted[2])?,
                ],
            );
            if seen.insert(op) {
                operations.push(op);
            }
        }
    }
    // The checks above make it a group modulo the new lattice: the new
    // lattice consists of translations of the crystal, and every operation
    // maps it onto itself.
    let symmetry = Symmetry::from_group(operations);
    // Which of the operations that differ by a centering translation comes
    // first depends on the cell the structure came from; a tabulated
    // setting is given in the order of the tables, so that a change of cell
    // and its inverse give back the operations in their order.
    Ok(match tables::match_operations(&symmetry) {
        Some(setting) => setting.symmetry.clone(),
        None => symmetry,
    })
}

/// Puts a value computed through a change of cell back on the number it is
/// within rounding of, as the source most likely wrote it: a fraction with
/// a denominator up to 24, or a decimal with up to 10 places. Values read
/// from a file then come back exactly after a change of cell and its
/// inverse.
pub(crate) fn tidy(value: f64) -> f64 {
    const ROUNDING: f64 = 8e-15;
    let tolerance = ROUNDING * value.abs().max(1.0);
    let near = |scale: f64| {
        let candidate = (value * scale).round() / scale;
        ((candidate - value).abs() <= tolerance).then_some(candidate + 0.0)
    };
    (1..=24)
        .map(f64::from)
        .find_map(near)
        .or_else(|| (1..=10).find_map(|places| near(10f64.powi(places))))
        .unwrap_or(value)
}

/// The cell parameters of `A·P`. A new basis vector that is a multiple of
/// one old one keeps that length, scaled, uncertainty included, and the
/// angle between two such vectors is the old angle (or 180° minus it);
/// the other lengths and angles come from the metric tensor `Pᵀ·G·P`, with
/// exact right and 60°/120° angles, and have no uncertainty: the
/// correlations it would need are not known.
pub(crate) fn transformed_params(old: &CellParams, transform: &CellTransform) -> CellParams {
    let p = transform
        .matrix()
        .map(|row| row.map(crate::Rational::to_f64));
    // For each new axis j, the old axis it lies along and the factor.
    let along: [Option<(usize, f64)>; 3] = [0, 1, 2].map(|j| {
        let nonzero: Vec<usize> = (0..3).filter(|&i| p[i][j] != 0.0).collect();
        match nonzero[..] {
            [i] => Some((i, p[i][j])),
            _ => None,
        }
    });
    let from_metric = along.iter().any(Option::is_none).then(|| {
        let g = Matrix3::from(old.metric()).transpose();
        let pm = Matrix3::from_fn(|i, j| p[i][j]);
        let g2 = pm.transpose() * g * pm;
        CellParams::from_metric([0, 1, 2].map(|i| [0, 1, 2].map(|j| g2[(i, j)])))
    });
    let computed = |param: CellParam| {
        let value = from_metric
            .expect("computed when an axis needs it")
            .get(param);
        Measured::exact(tidy(value.value()))
    };
    let lengths = [old.a, old.b, old.c];
    let angles = [old.alpha, old.beta, old.gamma];
    let length_param = [CellParam::A, CellParam::B, CellParam::C];
    // The angle between axes i and k is the (3 − i − k)-th.
    let angle_param = [CellParam::Alpha, CellParam::Beta, CellParam::Gamma];
    let length = |j: usize| {
        let Some((i, factor)) = along[j] else {
            return computed(length_param[j]);
        };
        let value = lengths[i];
        let factor = factor.abs();
        if factor == 1.0 {
            return value;
        }
        let scaled = tidy(value.value() * factor);
        match value.su() {
            Some(su) => Measured::with_su(scaled, su * factor),
            None => Measured::exact(scaled),
        }
    };
    let angle = |j: usize, k: usize| {
        let (Some((i, fi)), Some((l, fl))) = (along[j], along[k]) else {
            return computed(angle_param[3 - j - k]);
        };
        let value = angles[3 - i - l];
        if fi.signum() == fl.signum() {
            return value;
        }
        let supplement = tidy(180.0 - value.value());
        match value.su() {
            Some(su) => Measured::with_su(supplement, su),
            None => Measured::exact(supplement),
        }
    };
    CellParams {
        a: length(0),
        b: length(1),
        c: length(2),
        alpha: angle(1, 2),
        beta: angle(0, 2),
        gamma: angle(0, 1),
    }
}

/// Describes the structure in the cell `(P, p)` (design §7.8): new basis
/// `A·P`, coordinates `x' = P⁻¹·(x − p)`, the operations of
/// [`transform_operations`]. The asymmetric unit stays the asymmetric unit:
/// the same sites with the same ids, labels and occupants, displacement
/// parameters and uncertainties carried through `P⁻¹`. Every atom keeps its
/// place relative to the new origin, so the lattice is oriented to put the
/// new basis vectors where `A·P` is (design §4.2).
pub(crate) fn change_basis(
    structure: &Structure,
    transform: &CellTransform,
) -> Result<Structure, TransformError> {
    let symmetry = transform_operations(&structure.symmetry, transform)?;
    let old = &structure.lattice;
    let params = transformed_params(old.params(), transform);
    let lattice = if is_scaling(transform) {
        let mut lattice = *old;
        lattice.set_params(params);
        lattice
    } else {
        let basis = old.matrix();
        let p = transform
            .matrix()
            .map(|row| row.map(crate::Rational::to_f64));
        let new_basis =
            [0, 1, 2].map(|j| [0, 1, 2].map(|k| (0..3).map(|i| p[i][j] * basis[i][k]).sum()));
        Lattice::oriented_as(params, new_basis).ok_or(TransformError::TooLarge)?
    };
    if !lattice.params().is_valid() {
        return Err(TransformError::TooLarge);
    }

    let inverse = cell_transform::mat_inverse(&transform.exact_matrix())
        .ok_or(TransformError::TooLarge)?
        .map(|row| row.map(Exact::to_f64));
    let origin = transform.origin().map(crate::Rational::to_f64);
    let old_reciprocal = old.params().reciprocal_lengths();
    let new_reciprocal = lattice.params().reciprocal_lengths();
    let sites = structure
        .sites
        .iter()
        .map(|site| {
            let d = [0, 1, 2].map(|i| site.position[i] - origin[i]);
            let position =
                [0, 1, 2].map(|i| tidy((0..3).map(|j| inverse[i][j] * d[j]).sum::<f64>()));
            let occupants = site
                .occupants
                .iter()
                .map(|occupant| Occupant {
                    adp: occupant.adp.map(|adp| {
                        tidy_adp(transform_adp(
                            adp,
                            &inverse,
                            &old_reciprocal,
                            &new_reciprocal,
                        ))
                    }),
                    ..occupant.clone()
                })
                .collect();
            Site {
                position,
                position_su: transform_su(site.position_su, &inverse),
                occupants,
                ..site.clone()
            }
        })
        .collect();

    // The type stays the same; the name the source gave stays as the
    // record of what the source wrote.
    Ok(Structure {
        lattice,
        symmetry,
        space_group: structure.space_group.clone(),
        sites,
        tolerances: structure.tolerances,
    })
}

/// Whether P only stretches the axes: diagonal, with positive entries. A
/// negative entry turns its axis around, and with it the cell in space.
fn is_scaling(transform: &CellTransform) -> bool {
    let m = transform.matrix();
    (0..3).all(|i| {
        (0..3).all(|j| {
            if i == j {
                m[i][j].numerator() > 0
            } else {
                m[i][j].is_zero()
            }
        })
    })
}

fn tidy_adp(adp: Adp) -> Adp {
    let tidied = |m: Measured| match m.su() {
        Some(su) => Measured::with_su(tidy(m.value()), su),
        None => Measured::exact(tidy(m.value())),
    };
    match adp {
        Adp::Isotropic(u) => Adp::Isotropic(u),
        Adp::Anisotropic(u) => Adp::Anisotropic(u.map(tidied)),
    }
}

/// Where the atoms of each site went through the change of cell that turned
/// `before` into `after` (design §7.11): the same atom, under its member
/// number and cell in the new cell.
pub(crate) fn rebase_map(
    before: &Structure,
    after: &Structure,
    transform: &CellTransform,
) -> IdMap {
    let inverse = cell_transform::mat_inverse(&transform.exact_matrix())
        .map(|m| m.map(|row| row.map(Exact::to_f64)));
    let Some(linear) = inverse else {
        return IdMap::reshaped(before.sites.iter().map(|s| s.id));
    };
    let origin = transform.origin().map(crate::Rational::to_f64);
    let shift = [0, 1, 2].map(|i| -(0..3).map(|j| linear[i][j] * origin[j]).sum::<f64>());
    let positions = |structure: &Structure, site: &Site| -> Vec<[f64; 3]> {
        expand::members(structure, site)
            .into_iter()
            .map(|(_, position)| position)
            .collect()
    };
    IdMap::with_fates(before.sites.iter().filter_map(|site| {
        let new = after.site(site.id)?;
        let rebasing = Rebasing::new(
            linear,
            shift,
            positions(before, site),
            positions(after, new),
        );
        Some((site.id, SiteFate::Rebased(Box::new(rebasing))))
    }))
}

/// The lattice `A·M`, oriented so that every atom stays where it was in
/// Cartesian space (design §4.2): a diagonal matrix keeps the orientation,
/// others turn the standard orientation of the new parameters onto the new
/// basis vectors.
fn supercell_lattice(old: &Lattice, m: &[[i64; 3]; 3]) -> Lattice {
    let params = supercell_params(old.params(), m);
    let mut lattice = *old;
    lattice.set_params(params);
    if is_diagonal(m) {
        return lattice;
    }
    let basis = old.matrix();
    let new_basis =
        [0, 1, 2].map(|j| [0, 1, 2].map(|k| (0..3).map(|i| m[i][j] as f64 * basis[i][k]).sum()));
    // The parameters come from these very vectors, so the rotation exists.
    Lattice::oriented_as(params, new_basis).unwrap_or(lattice)
}

fn is_diagonal(m: &[[i64; 3]; 3]) -> bool {
    (0..3).all(|i| (0..3).all(|j| i == j || m[i][j] == 0))
}

/// The cell parameters of the lattice `A·M`. A diagonal matrix multiplies
/// the lengths, uncertainties included, and keeps the angles exactly; other
/// matrices go through the metric tensor `Mᵀ·G·M`, with right and 60°/120°
/// angles kept exact.
fn supercell_params(old: &CellParams, m: &[[i64; 3]; 3]) -> CellParams {
    let scale = |value: Measured, n: i64| {
        let n = n as f64;
        match value.su() {
            Some(su) => Measured::with_su(value.value() * n, su * n),
            None => Measured::exact(value.value() * n),
        }
    };
    if is_diagonal(m) {
        return CellParams {
            a: scale(old.a, m[0][0]),
            b: scale(old.b, m[1][1]),
            c: scale(old.c, m[2][2]),
            ..*old
        };
    }
    let g = Matrix3::from(old.metric()).transpose();
    let mm = Matrix3::from_fn(|i, j| m[i][j] as f64);
    let g2 = mm.transpose() * g * mm;
    CellParams::from_metric([0, 1, 2].map(|i| [0, 1, 2].map(|j| g2[(i, j)])))
}

/// The (i, j) pairs of the six stored components: U11, U22, U33, U12, U13,
/// U23.
const PAIRS: [(usize, usize); 6] = [(0, 0), (1, 1), (2, 2), (0, 1), (0, 2), (1, 2)];

/// Carries displacement parameters through a linear map `T` of fractional
/// displacements (a rotation of the operation, or `M⁻¹` of a change of
/// basis): `B' = T·B·Tᵀ` with `B = N·U·N`, `N = diag(a*, b*, c*)`. The
/// coefficients are snapped to integers when within rounding, so that a
/// component that only moves keeps its exact value; an uncertainty adds in
/// quadrature over the components it comes from, and is lost when one of
/// them has none.
pub(crate) fn transform_adp(adp: Adp, t: &[[f64; 3]; 3], from: &[f64; 3], to: &[f64; 3]) -> Adp {
    let Adp::Anisotropic(u) = adp else {
        return adp;
    };
    let coefficient = |(i, j): (usize, usize), (k, l): (usize, usize)| {
        let term = |k: usize, l: usize| t[i][k] * t[j][l] * from[k] * from[l] / (to[i] * to[j]);
        let c = if k == l {
            term(k, l)
        } else {
            term(k, l) + term(l, k)
        };
        snap(c)
    };
    let next = PAIRS.map(|out| {
        let mut value = 0.0;
        let mut variance = Some(0.0);
        let mut terms = 0;
        for (index, input) in PAIRS.iter().enumerate() {
            let c = coefficient(out, *input);
            if c == 0.0 {
                continue;
            }
            terms += 1;
            value += c * u[index].value();
            variance = variance
                .zip(u[index].su())
                .map(|(v, su)| v + (c * su).powi(2));
        }
        match variance {
            Some(v) if terms > 0 => Measured::with_su(value, v.sqrt()),
            _ => Measured::exact(value),
        }
    });
    Adp::Anisotropic(next)
}

/// Carries coordinate uncertainties through a linear map, in quadrature
/// (correlations are not known). A coordinate that comes from one without
/// an uncertainty has none.
pub(crate) fn transform_su(su: [Option<f64>; 3], t: &[[f64; 3]; 3]) -> [Option<f64>; 3] {
    [0, 1, 2].map(|i| {
        let mut variance = 0.0;
        for (j, su) in su.iter().enumerate() {
            let c = snap(t[i][j]);
            if c == 0.0 {
                continue;
            }
            variance += (c * (*su)?).powi(2);
        }
        Some(variance.sqrt())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rotation;
    use crate::sample::{self, SampleKey};
    use crate::{Derived, Species};

    #[test]
    fn rutile_expands_to_one_site_per_atom() {
        let rutile = sample::load(SampleKey::Rutile).structure;
        let Expanded { structure, splits } = expand_to_p1(&rutile);
        assert!(structure.symmetry.is_p1());
        assert_eq!(structure.space_group, p1_space_group());
        let labels: Vec<&str> = structure.sites.iter().map(Site::label).collect();
        assert_eq!(labels, ["Ti1_1", "Ti1_2", "O1_1", "O1_2", "O1_3", "O1_4"]);
        // New ids follow the old ones.
        assert_eq!(structure.sites[0].id, SiteId(3));
        assert_eq!(
            splits,
            [
                (SiteId(1), vec![SiteId(3), SiteId(4)]),
                (SiteId(2), (5..9).map(SiteId).collect())
            ]
        );
        // The same atoms in the cell, in the same order.
        let before = Derived::compute(&rutile);
        let after = Derived::compute(&structure);
        let positions = |derived: &Derived| -> Vec<[f64; 3]> {
            derived
                .orbits
                .iter()
                .flat_map(|o| o.positions.clone())
                .collect()
        };
        assert_eq!(positions(&before), positions(&after));
        assert_eq!(after.atoms.len(), before.atoms.len());
        assert_eq!(structure.validate(), Ok(()));
    }

    #[test]
    fn a_single_atom_keeps_its_label() {
        let srtio3 = sample::load(SampleKey::StrontiumTitanate).structure;
        let labels: Vec<String> = expand_to_p1(&srtio3)
            .structure
            .sites
            .iter()
            .map(|s| s.label().to_string())
            .collect();
        assert_eq!(labels, ["Sr1", "Ti1", "O1_1", "O1_2", "O1_3"]);
    }

    #[test]
    fn displacement_parameters_follow_the_operations() {
        // Rutile's O on 4f (x, x, 0) with U11 = U22 ≠ U33 and U12: the image
        // at -x+1/2, x+1/2, 1/2 (rotated by the fourfold screw axis) has U12
        // with the opposite sign.
        let mut rutile = sample::load(SampleKey::Rutile).structure;
        let u = [
            Measured::with_su(0.0070, 0.0002),
            Measured::with_su(0.0070, 0.0002),
            Measured::with_su(0.0045, 0.0003),
            Measured::with_su(0.0021, 0.0002),
            Measured::exact(0.0),
            Measured::exact(0.0),
        ];
        rutile.sites[1].occupants[0].adp = Some(Adp::Anisotropic(u));
        rutile.sites[1].position_su = [Some(0.0002), Some(0.0002), None];
        let expanded = expand_to_p1(&rutile).structure;
        let images: Vec<&Site> = expanded
            .sites
            .iter()
            .filter(|s| s.element() == "O")
            .collect();
        for image in &images {
            let Some(Adp::Anisotropic(v)) = image.occupants[0].adp else {
                panic!("anisotropic");
            };
            // The diagonal moves between equal axes and stays exact.
            assert_eq!(v[0], u[0]);
            assert_eq!(v[2], u[2]);
            assert_eq!(v[3].value().abs(), 0.0021);
            assert_eq!(v[3].su(), Some(0.0002));
            assert_eq!(image.position_su, [Some(0.0002), Some(0.0002), None]);
        }
        let signs: HashSet<bool> = images
            .iter()
            .map(|s| match s.occupants[0].adp {
                Some(Adp::Anisotropic(v)) => v[3].value() > 0.0,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(signs.len(), 2, "both signs of U12 occur");
    }

    #[test]
    fn the_rutile_supercell_matches_the_built_in_one() {
        let rutile = sample::load(SampleKey::Rutile).structure;
        let built_in = sample::load(SampleKey::RutileSupercell).structure;
        let supercell = make_supercell(&rutile, [[2, 0, 0], [0, 2, 0], [0, 0, 2]]).unwrap();
        assert_eq!(supercell.lattice, built_in.lattice);
        assert_eq!(supercell.sites.len(), 48);
        for (made, expected) in supercell.sites.iter().zip(&built_in.sites) {
            assert_eq!(made.position, expected.position);
            assert_eq!(made.element(), expected.element());
        }
        let labels: Vec<&str> = supercell.sites.iter().take(7).map(Site::label).collect();
        assert_eq!(
            labels,
            ["Ti1_1", "Ti1_2", "O1_1", "O1_2", "O1_3", "O1_4", "Ti1_3"]
        );
        assert_eq!(supercell.validate(), Ok(()));
        let derived = Derived::compute(&supercell);
        assert_eq!(derived.atoms_in_cell(), 48);
        let density = derived.density.unwrap();
        assert!((density - 4.25).abs() < 0.01, "{density}");
    }

    #[test]
    fn other_matrices() {
        let srtio3 = sample::load(SampleKey::StrontiumTitanate).structure;
        // 3 × 1 × 1: the a axis divides by 3 only.
        let long = make_supercell(&srtio3, [[3, 0, 0], [0, 1, 0], [0, 0, 1]]).unwrap();
        assert_eq!(long.sites.len(), 15);
        assert_eq!(long.lattice.params().a.value(), 3.0 * 3.905);
        assert!(
            long.sites
                .iter()
                .all(|s| [0.0, 0.5].contains(&s.position[1]))
        );
        // √2 × √2 × 1, rotated by 45°: twice the atoms, a and b √2 longer,
        // angles still exactly 90°.
        let rotated = make_supercell(&srtio3, [[1, 1, 0], [-1, 1, 0], [0, 0, 1]]).unwrap();
        assert_eq!(rotated.sites.len(), 10);
        let params = rotated.lattice.params();
        assert!((params.a.value() - 3.905 * 2f64.sqrt()).abs() < 1e-12);
        assert_eq!(params.gamma.value(), 90.0);
        assert_eq!(params.alpha.value(), 90.0);
        // The new basis vectors are a − b and a + b where they were: the
        // lattice is turned by −45° about z, not put in standard orientation.
        let basis = rotated.lattice.matrix();
        let expected = [[3.905, -3.905, 0.0], [3.905, 3.905, 0.0], [0.0, 0.0, 3.905]];
        for (got, want) in basis.iter().flatten().zip(expected.iter().flatten()) {
            assert!((got - want).abs() < 1e-12, "{basis:?}");
        }
        let derived = Derived::compute(&rotated);
        assert_eq!(derived.atoms_in_cell(), 10);
        let density = Derived::compute(&srtio3).density.unwrap();
        assert!((derived.density.unwrap() - density).abs() < 1e-9);
        // Partial occupancy stays partial.
        let mut mixed = srtio3.clone();
        mixed.sites[0].occupants[0].occupancy = Measured::exact(0.5);
        mixed.sites[0].occupants.push(Occupant::new(
            "Ba1",
            Species::element("Ba"),
            Measured::exact(0.5),
        ));
        let cell = make_supercell(&mixed, [[2, 0, 0], [0, 1, 0], [0, 0, 1]]).unwrap();
        assert_eq!(cell.sites[0].occupants.len(), 2);
        assert_eq!(cell.sites[0].occupants[1].label, "Ba1_1");

        assert_eq!(
            make_supercell(&srtio3, [[1, 0, 0], [0, 1, 0], [0, 0, 0]]),
            Err(SupercellError::NotPositive { determinant: 0 })
        );
        assert!(make_supercell(&srtio3, [[-1, 0, 0], [0, 1, 0], [0, 0, 1]]).is_err());
    }

    #[test]
    fn left_handed_lattices_turn_right_handed_in_place() {
        let rutile = sample::load(SampleKey::Rutile).structure;
        assert!(matches!(right_handed(&rutile), Cow::Borrowed(_)));
        let mut mirrored = rutile.clone();
        let rotation = Rotation::new([[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]).unwrap();
        mirrored.lattice =
            Lattice::with_orientation(*rutile.lattice.params(), Orientation::new(rotation));
        // Shift the O site so that coordinates of both signs occur.
        mirrored.sites[1].position = [0.3048, 0.3048, 0.0];
        let turned = right_handed(&mirrored).into_owned();
        assert!(!turned.lattice.is_left_handed());
        assert_eq!(turned.lattice.params(), mirrored.lattice.params());
        assert_eq!(turned.sites[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(turned.sites[1].position, [1.0 - 0.3048, 1.0 - 0.3048, 0.0]);
        assert!(
            turned.sites[0]
                .position
                .iter()
                .all(|v| v.is_sign_positive())
        );
        // The screw axis -y+1/2,x+1/2,z+1/2 keeps its translation modulo 1.
        assert!(
            turned
                .symmetry
                .operations()
                .contains(&SymOp::parse("-y+1/2,x+1/2,z+1/2").unwrap())
        );
        assert_eq!(turned.symmetry.operations().len(), 16);
        // The same atoms in Cartesian space, up to lattice translations.
        let atoms = |structure: &Structure| -> Vec<[f64; 3]> {
            crate::orbits(structure)
                .iter()
                .flat_map(|o| o.positions.clone())
                .map(|x| structure.lattice.to_cartesian(x))
                .collect()
        };
        let before = atoms(&mirrored);
        let after = atoms(&turned);
        assert_eq!(before.len(), after.len());
        for r in after {
            assert!(before.iter().any(|a| {
                let d = mirrored
                    .lattice
                    .to_fractional([0, 1, 2].map(|i| r[i] - a[i]));
                d.iter().all(|v| (v - v.round()).abs() < 1e-9)
            }));
        }
    }

    #[test]
    fn supercells_keep_the_atoms_in_place() {
        // Rutile with its cell given as left-handed vectors turned about z.
        let mut rutile = sample::load(SampleKey::Rutile).structure;
        let (s, c) = 0.3f64.sin_cos();
        let rotation = Rotation::new([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, -1.0]]).unwrap();
        rutile.lattice =
            Lattice::with_orientation(*rutile.lattice.params(), crate::Orientation::new(rotation));
        assert!(rutile.lattice.is_left_handed());
        let before = Derived::compute(&rutile);
        let cartesian = |structure: &Structure, derived: &Derived| -> Vec<[f64; 3]> {
            derived
                .orbits
                .iter()
                .flat_map(|o| o.positions.clone())
                .map(|x| structure.lattice.to_cartesian(x))
                .collect()
        };
        let atoms = cartesian(&rutile, &before);
        for matrix in [
            [[2, 0, 0], [0, 1, 0], [0, 0, 3]],
            [[1, 1, 0], [-1, 1, 0], [0, 0, 1]],
            [[1, 0, 1], [0, 1, 0], [-1, 0, 1]],
        ] {
            let supercell = make_supercell(&rutile, matrix).unwrap();
            assert!(supercell.lattice.is_left_handed(), "{matrix:?}");
            if matrix[0][1] == 0 && matrix[0][2] == 0 {
                assert_eq!(
                    supercell.lattice.orientation(),
                    rutile.lattice.orientation()
                );
            }
            // Every atom of the supercell sits on an atom of the original
            // crystal, some lattice translation away.
            let derived = Derived::compute(&supercell);
            for r in cartesian(&supercell, &derived) {
                let found = atoms.iter().any(|a| {
                    let d = rutile.lattice.to_fractional([0, 1, 2].map(|i| r[i] - a[i]));
                    d.iter().all(|v| (v - v.round()).abs() < 1e-9)
                });
                assert!(found, "{matrix:?}: {r:?}");
            }
        }
    }
}
