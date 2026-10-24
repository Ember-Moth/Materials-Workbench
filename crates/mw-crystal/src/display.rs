//! The display range (design §5.5): which part of the infinite crystal the
//! view shows, a block of unit cells or the atoms within a distance of a
//! center, and the atoms and bonds it shows. It is view state: it changes no
//! structure and is not undone.

use std::fmt;

use crate::{AtomFate, AtomRef, Bond, Derived, IdMap, Orbit, Polyhedron, Structure, bonds};

/// Atoms beyond which a range is refused: the viewport draws 100 000 atoms
/// with their bonds at about 30 frames a second (viewport design §8.2).
pub const MAX_DISPLAY_ATOMS: usize = 100_000;

/// What part of the crystal the view shows (design §5.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DisplayRange {
    /// `counts` unit cells along a, b and c from the cell at the origin.
    /// With `boundary`, atoms on the range's faces, edges and corners are
    /// repeated on the opposite ones.
    Cells { counts: [u32; 3], boundary: bool },
    /// The atoms within `radius` Å of the center.
    Sphere { center: SphereCenter, radius: f64 },
}

/// One unit cell with its boundary atoms, as v0.1 showed every structure.
impl Default for DisplayRange {
    fn default() -> Self {
        Self::Cells {
            counts: [1; 3],
            boundary: true,
        }
    }
}

/// The center of a spherical display range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SphereCenter {
    Atom(AtomRef),
    /// Fractional coordinates.
    Point([f64; 3]),
}

/// Why a display range cannot be shown for a structure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DisplayRangeError {
    /// A count of cells is zero.
    NoCells,
    /// The radius is not a positive number.
    InvalidRadius,
    /// The structure has no such atom to center on.
    UnknownCenter(AtomRef),
    /// The range would show about `atoms` atoms, more than
    /// [`MAX_DISPLAY_ATOMS`].
    TooManyAtoms { atoms: usize },
}

impl fmt::Display for DisplayRangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCells => write!(f, "the range must have at least one cell along each axis"),
            Self::InvalidRadius => write!(f, "the radius must be a positive number of Å"),
            Self::UnknownCenter(atom) => write!(
                f,
                "the structure has no atom {} of site {} to center on",
                atom.member, atom.site.0
            ),
            Self::TooManyAtoms { atoms } => write!(
                f,
                "the range would show about {atoms} atoms, more than {MAX_DISPLAY_ATOMS}"
            ),
        }
    }
}

impl std::error::Error for DisplayRangeError {}

/// One atom of the displayed structure, in Cartesian Å.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct DisplayAtom {
    pub atom: AtomRef,
    pub element: String,
    pub position: [f64; 3],
}

/// The atoms a display range shows, and the bonds between them; in the
/// polyhedra model, also the coordination polyhedra (design §5.6).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Displayed {
    /// By site, orbit member, then cell; in the polyhedra model, followed by
    /// the ligands outside the range that complete the polyhedra.
    pub atoms: Vec<DisplayAtom>,
    /// Bonds with both atoms shown, as index pairs into `atoms`; in the
    /// polyhedra model, without those the polyhedra show.
    pub bonds: Vec<[usize; 2]>,
    /// Coordination polyhedra, only in the polyhedra model
    /// ([`Displayed::with_polyhedra`]).
    pub polyhedra: Vec<Polyhedron>,
}

impl Displayed {
    /// The atoms of the structure that `range` shows, and the bonds of the
    /// crystal (once per unit cell, design §5.3) between them. A sphere
    /// around an atom the structure does not have shows nothing; ranges are
    /// checked with [`DisplayRange::check`] first.
    pub fn new(
        structure: &Structure,
        orbits: &[Orbit],
        bonds: &[Bond],
        range: &DisplayRange,
    ) -> Self {
        let atoms = atoms(structure, orbits, range);
        let bonds = bonds::display_bonds(&atoms, bonds);
        Self {
            atoms,
            bonds,
            polyhedra: Vec::new(),
        }
    }
}

impl Derived {
    /// What `range` shows of the structure these data were derived from.
    pub fn display(&self, structure: &Structure, range: &DisplayRange) -> Displayed {
        Displayed::new(structure, &self.orbits, &self.bonds, range)
    }
}

/// Fractional coordinates of an orbit member translated by its cell.
pub(crate) fn position(orbits: &[Orbit], atom: AtomRef) -> Option<[f64; 3]> {
    let orbit = orbits.iter().find(|orbit| orbit.site == atom.site)?;
    let p = orbit.positions.get(usize::from(atom.member))?;
    Some([0, 1, 2].map(|i| p[i] + f64::from(atom.cell[i])))
}

/// The position tolerance in fractional coordinates along each axis: the
/// distance to the lattice planes, so that it holds in skew cells too.
fn fractional_tolerance(structure: &Structure) -> [f64; 3] {
    let reciprocal = structure.lattice.params().reciprocal_lengths();
    reciprocal.map(|r| structure.tolerances.position * r)
}

/// The cells whose copy of the coordinate `p` lies in `[low, high]`.
fn cells_between(p: f64, low: f64, high: f64) -> std::ops::RangeInclusive<i32> {
    let first = (low - p).ceil();
    let last = (high - p).floor();
    // Out of i32's range only for ranges far beyond any that is drawn.
    (first.max(f64::from(i32::MIN)) as i32)..=(last.min(f64::from(i32::MAX)) as i32)
}

/// Along each axis, the cells in which the coordinate `p` of a member is
/// shown by a block of cells.
fn cell_ranges(
    p: [f64; 3],
    counts: [u32; 3],
    boundary: bool,
    tolerance: [f64; 3],
) -> [std::ops::RangeInclusive<i32>; 3] {
    [0, 1, 2].map(|i| {
        let n = f64::from(counts[i]);
        if boundary {
            cells_between(p[i], -tolerance[i], n + tolerance[i])
        } else {
            // Orbit members lie in [0, 1), so each cell has one copy.
            0..=(counts[i].min(i32::MAX as u32) as i32 - 1)
        }
    })
}

fn len(range: &std::ops::RangeInclusive<i32>) -> usize {
    if range.is_empty() {
        0
    } else {
        (i64::from(*range.end()) - i64::from(*range.start()) + 1) as usize
    }
}

impl DisplayRange {
    /// Fractional coordinates of the sphere's center; `None` for a block of
    /// cells, or an atom the orbits do not have.
    pub fn center(&self, orbits: &[Orbit]) -> Option<[f64; 3]> {
        match *self {
            Self::Cells { .. } => None,
            Self::Sphere {
                center: SphereCenter::Point(point),
                ..
            } => Some(point),
            Self::Sphere {
                center: SphereCenter::Atom(atom),
                ..
            } => position(orbits, atom),
        }
    }

    /// How many atoms the range shows of the structure, or why it cannot be
    /// shown. Exact for cells; estimated from the volume for a sphere.
    pub fn check(
        &self,
        structure: &Structure,
        orbits: &[Orbit],
    ) -> Result<usize, DisplayRangeError> {
        let atoms = match *self {
            Self::Cells { counts, boundary } => {
                if counts.contains(&0) {
                    return Err(DisplayRangeError::NoCells);
                }
                let tolerance = fractional_tolerance(structure);
                let mut atoms = 0usize;
                for p in orbits.iter().flat_map(|orbit| &orbit.positions) {
                    let images = cell_ranges(*p, counts, boundary, tolerance)
                        .iter()
                        .map(len)
                        .fold(1usize, usize::saturating_mul);
                    atoms = atoms.saturating_add(images);
                }
                atoms
            }
            Self::Sphere { center, radius } => {
                if !(radius.is_finite() && radius > 0.0) {
                    return Err(DisplayRangeError::InvalidRadius);
                }
                if let SphereCenter::Atom(atom) = center
                    && position(orbits, atom).is_none()
                {
                    return Err(DisplayRangeError::UnknownCenter(atom));
                }
                let in_cell: usize = orbits.iter().map(|orbit| orbit.positions.len()).sum();
                let r = radius + structure.tolerances.position;
                let volume = structure.lattice.params().volume();
                let sphere = 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
                // At least the center itself.
                (in_cell as f64 * sphere / volume)
                    .ceil()
                    .clamp(1.0, usize::MAX as f64) as usize
            }
        };
        if atoms > MAX_DISPLAY_ATOMS {
            return Err(DisplayRangeError::TooManyAtoms { atoms });
        }
        Ok(atoms)
    }

    /// The range after an edit (design §7.11): a sphere around an atom
    /// follows the atom. Once the atom is gone, or its reference may name
    /// another atom, the sphere stays where it was: around `before`, the
    /// center's fractional coordinates before the edit.
    pub fn remap(self, ids: &IdMap, before: Option<[f64; 3]>) -> Self {
        let Self::Sphere {
            center: SphereCenter::Atom(atom),
            radius,
        } = self
        else {
            return self;
        };
        let center = match (ids.atom(atom), before) {
            (AtomFate::Kept(atom), _) => SphereCenter::Atom(atom),
            (AtomFate::Site(_) | AtomFate::Removed, Some(point)) => SphereCenter::Point(point),
            (AtomFate::Site(_) | AtomFate::Removed, None) => SphereCenter::Atom(atom),
        };
        Self::Sphere { center, radius }
    }

    /// The same range around a fixed point instead of an atom, for showing
    /// an edit in progress: the region stays put while its atom moves.
    pub fn fixed(self, orbits: &[Orbit]) -> Self {
        match (self, self.center(orbits)) {
            (Self::Sphere { radius, .. }, Some(point)) => Self::Sphere {
                center: SphereCenter::Point(point),
                radius,
            },
            _ => self,
        }
    }
}

/// The atoms `range` shows: for a block of cells, each orbit member once
/// per cell plus its boundary images; for a sphere, every copy of a member
/// within the radius (and the position tolerance) of the center.
pub(crate) fn atoms(
    structure: &Structure,
    orbits: &[Orbit],
    range: &DisplayRange,
) -> Vec<DisplayAtom> {
    let lattice = &structure.lattice;
    let tolerance = fractional_tolerance(structure);
    let reciprocal = lattice.params().reciprocal_lengths();
    let center = range.center(orbits);
    let mut atoms = Vec::new();
    for orbit in orbits {
        let Some(site) = structure.site(orbit.site) else {
            continue;
        };
        let element = site.element();
        for (member, p) in orbit.positions.iter().enumerate() {
            let mut add = |cell: [i32; 3]| {
                let x = [0, 1, 2].map(|i| p[i] + f64::from(cell[i]));
                atoms.push(DisplayAtom {
                    atom: AtomRef::new(site.id, member as u16, cell),
                    element: element.to_string(),
                    position: lattice.to_cartesian(x),
                });
            };
            match *range {
                DisplayRange::Cells { counts, boundary } => {
                    let [xs, ys, zs] = cell_ranges(*p, counts, boundary, tolerance);
                    for x in xs {
                        for y in ys.clone() {
                            for z in zs.clone() {
                                add([x, y, z]);
                            }
                        }
                    }
                }
                DisplayRange::Sphere { radius, .. } => {
                    let Some(c) = center else {
                        continue;
                    };
                    let reach = radius + structure.tolerances.position;
                    // The sphere spans reach · |a*| along a, and so on.
                    let [xs, ys, zs] = [0, 1, 2].map(|i| {
                        let extent = reach * reciprocal[i];
                        cells_between(p[i], c[i] - extent, c[i] + extent)
                    });
                    for x in xs {
                        for y in ys.clone() {
                            for z in zs.clone() {
                                let cell = [x, y, z];
                                let d = [0, 1, 2].map(|i| p[i] + f64::from(cell[i]) - c[i]);
                                let r = lattice.to_cartesian(d);
                                if (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt() <= reach {
                                    add(cell);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    atoms
}

#[cfg(test)]
impl Derived {
    /// The default display, one cell with its boundary images: atoms with
    /// cell offsets, for tests that follow references through edits.
    pub(crate) fn one_cell(&self, structure: &Structure) -> Vec<DisplayAtom> {
        self.display(structure, &DisplayRange::default()).atoms
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{self, SampleKey};
    use crate::{CellParams, Edit, Lattice, SiteId, Symmetry, atom_ref::distance};

    fn cells(counts: [u32; 3], boundary: bool) -> DisplayRange {
        DisplayRange::Cells { counts, boundary }
    }

    fn sphere(center: SphereCenter, radius: f64) -> DisplayRange {
        DisplayRange::Sphere { center, radius }
    }

    /// Checks the range, displays it, and checks what was displayed: the
    /// count `check` gave for cells, every atom where its reference says,
    /// and every bond once, between shown atoms at a bonding distance.
    fn show(structure: &Structure, range: DisplayRange) -> Displayed {
        let derived = Derived::compute(structure);
        let count = range.check(structure, &derived.orbits).unwrap();
        let shown = derived.display(structure, &range);
        if let DisplayRange::Cells { .. } = range {
            assert_eq!(shown.atoms.len(), count);
        }
        let mut seen = std::collections::HashSet::new();
        for atom in &shown.atoms {
            assert!(seen.insert(atom.atom), "{:?} twice", atom.atom);
            let r = derived.cartesian(&structure.lattice, atom.atom).unwrap();
            assert!(distance(r, atom.position) < 1e-9, "{atom:?}");
        }
        let mut pairs = std::collections::HashSet::new();
        for &[i, j] in &shown.bonds {
            assert!(pairs.insert((i.min(j), i.max(j))), "bond {i}–{j} twice");
            let length = distance(shown.atoms[i].position, shown.atoms[j].position);
            assert!(length < 3.0, "{length}");
        }
        shown
    }

    fn count_of(shown: &Displayed, element: &str) -> usize {
        shown.atoms.iter().filter(|a| a.element == element).count()
    }

    #[test]
    fn one_cell_with_its_boundary_atoms_by_default() {
        let srtio3 = sample::load(SampleKey::StrontiumTitanate).structure;
        let shown = show(&srtio3, DisplayRange::default());
        // Eight corner Sr, one Ti, six face-center O.
        assert_eq!(shown.atoms.len(), 8 + 1 + 6);
        assert!(
            shown
                .atoms
                .iter()
                .all(|a| a.position.iter().all(|v| (0.0..=3.905 + 1e-9).contains(v)))
        );
        let rutile = sample::load(SampleKey::Rutile).structure;
        assert_eq!(show(&rutile, DisplayRange::default()).atoms.len(), 15);
    }

    #[test]
    fn blocks_of_cells_repeat_every_member() {
        let rutile = sample::load(SampleKey::Rutile).structure;
        // Without boundary atoms, each cell holds the six atoms of the cell.
        let shown = show(&rutile, cells([2, 3, 1], false));
        assert_eq!(shown.atoms.len(), 6 * 6);
        for atom in &shown.atoms {
            let [x, y, z] = atom.atom.cell;
            assert!((0..2).contains(&x) && (0..3).contains(&y) && z == 0);
        }

        let srtio3 = sample::load(SampleKey::StrontiumTitanate).structure;
        let shown = show(&srtio3, cells([2, 2, 2], true));
        // 3 × 3 × 3 corners, 2 × 2 × 2 body centers; each face-center O on
        // three planes along its normal and two along the others.
        assert_eq!(count_of(&shown, "Sr"), 27);
        assert_eq!(count_of(&shown, "Ti"), 8);
        assert_eq!(count_of(&shown, "O"), 3 * 12);
        // Every Ti keeps its six O; every O its four Sr.
        assert_eq!(shown.bonds.len(), 8 * 6 + 36 * 4);
    }

    #[test]
    fn boundary_atoms_follow_the_distance_to_the_faces() {
        // A monoclinic cell, β = 120°: an atom 0.005 Å from the bc face is
        // 0.005 / sin β Å away along a, beyond a tolerance taken along a.
        let lattice = Lattice::new(CellParams::new(4.0, 5.0, 6.0, 90.0, 120.0, 90.0));
        let spacing = 4.0 * 120f64.to_radians().sin();
        let x = 0.005 / spacing;
        let structure = Structure::new(
            lattice,
            Symmetry::p1(),
            vec![crate::Site::single(1, "C1", "C", [x, 0.5, 0.5])],
        );
        let shown = show(&structure, DisplayRange::default());
        assert_eq!(shown.atoms.len(), 2);
        assert_eq!(shown.atoms[1].atom.cell, [1, 0, 0]);
        // Twice the tolerance from the face: in the cell only.
        let mut farther = structure.clone();
        farther.sites[0].position[0] = 2.0 * 0.01 / spacing;
        assert_eq!(show(&farther, DisplayRange::default()).atoms.len(), 1);
    }

    #[test]
    fn spheres_show_the_shells_around_their_center() {
        let sample = sample::load(SampleKey::StrontiumTitanate);
        let srtio3 = &sample.structure;
        let ti = AtomRef::new(sample.derived.orbits[1].site, 0, [0; 3]);
        // Around the Ti: six O at a/2, eight Sr at a·√3/2, six Ti at a.
        let around = |radius| show(srtio3, sphere(SphereCenter::Atom(ti), radius));
        assert_eq!(around(2.0).atoms.len(), 1 + 6);
        let shown = around(3.4);
        assert_eq!(shown.atoms.len(), 1 + 6 + 8);
        assert!(shown.atoms.iter().any(|a| a.atom == ti));
        assert_eq!(around(3.95).atoms.len(), 1 + 6 + 8 + 6);
        // The same around the Ti's place, as a point; and an image of it.
        let point = show(srtio3, sphere(SphereCenter::Point([0.5, 0.5, 0.5]), 3.4));
        assert_eq!(point.atoms, shown.atoms);
        let image = show(
            srtio3,
            sphere(SphereCenter::Atom(ti.translated([2, -1, 0])), 3.4),
        );
        assert_eq!(image.atoms.len(), shown.atoms.len());
        assert!(image.atoms.iter().all(|a| a.atom.cell[0] >= 1));

        // The Ti bonds to its six O, and each O to the four Sr of its face.
        assert_eq!(shown.bonds.len(), 6 + 6 * 4);

        // The estimate is about right: 5 atoms per 59.5 Å³.
        let estimate = sphere(SphereCenter::Atom(ti), 3.4)
            .check(srtio3, &sample.derived.orbits)
            .unwrap();
        assert!((10..=20).contains(&estimate), "{estimate}");
    }

    #[test]
    fn ranges_that_cannot_be_shown_are_refused() {
        let sample = sample::load(SampleKey::Rutile);
        let (rutile, orbits) = (&sample.structure, &sample.derived.orbits);
        let check = |range: DisplayRange| range.check(rutile, orbits);
        assert_eq!(
            check(cells([1, 0, 1], true)),
            Err(DisplayRangeError::NoCells)
        );
        let ti = AtomRef::new(SiteId(1), 0, [0; 3]);
        for radius in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                check(sphere(SphereCenter::Atom(ti), radius)),
                Err(DisplayRangeError::InvalidRadius)
            );
        }
        let missing = AtomRef::new(SiteId(1), 5, [0; 3]);
        assert_eq!(
            check(sphere(SphereCenter::Atom(missing), 5.0)),
            Err(DisplayRangeError::UnknownCenter(missing))
        );
        // Ten by ten by ten cells are fine; fifty are 750 000 atoms.
        assert_eq!(check(cells([10, 10, 10], false)), Ok(6000));
        assert_eq!(
            check(cells([50, 50, 50], false)),
            Err(DisplayRangeError::TooManyAtoms { atoms: 750_000 })
        );
        assert!(matches!(
            check(cells([u32::MAX; 3], true)),
            Err(DisplayRangeError::TooManyAtoms { .. })
        ));
        assert!(matches!(
            check(sphere(SphereCenter::Point([0.0; 3]), 1e6)),
            Err(DisplayRangeError::TooManyAtoms { .. })
        ));
    }

    #[test]
    fn large_ranges_stay_fast() {
        // 10 × 10 × 10 rutile cells: 6000 atoms, the boundary planes of the
        // Ti at the origin and of the O at z = 0 on top.
        let sample = sample::load(SampleKey::Rutile);
        let range = cells([10, 10, 10], true);
        assert_eq!(
            range.check(&sample.structure, &sample.derived.orbits),
            Ok(6531)
        );
        let started = std::time::Instant::now();
        let shown = sample.derived.display(&sample.structure, &range);
        let elapsed = started.elapsed();
        assert_eq!(shown.atoms.len(), 6531);
        // The 1000 body-center Ti keep all six of their O.
        assert!(shown.bonds.len() > 1000 * 6, "{}", shown.bonds.len());
        assert!(elapsed.as_secs_f64() < 2.0, "{elapsed:?}");
    }

    #[test]
    fn spheres_around_atoms_follow_them_through_edits() {
        let sample = sample::load(SampleKey::Rutile);
        let rutile = sample.structure;
        let o = AtomRef::new(SiteId(2), 1, [0, 1, 0]);
        let range = sphere(SphereCenter::Atom(o), 4.0);
        let before = range.center(&sample.derived.orbits).unwrap();

        // Converting to P1: the same atom under its new site.
        let applied = Edit::ExpandToP1.apply(&rutile).unwrap();
        let after = Derived::compute(&applied.structure);
        let remapped = range.remap(&applied.ids, Some(before));
        let DisplayRange::Sphere {
            center: SphereCenter::Atom(atom),
            radius,
        } = remapped
        else {
            panic!("{remapped:?}")
        };
        assert_ne!(atom.site, o.site);
        assert_eq!(atom.cell, o.cell);
        assert_eq!(radius, 4.0);
        assert_eq!(remapped.center(&after.orbits), Some(before));

        // Removing its site: a point where the atom was.
        let applied = Edit::RemoveSite { site: SiteId(2) }.apply(&rutile).unwrap();
        assert_eq!(
            range.remap(&applied.ids, Some(before)),
            sphere(SphereCenter::Point(before), 4.0)
        );

        // While an edit is shown, the region stays where it is.
        assert_eq!(
            range.fixed(&sample.derived.orbits),
            sphere(SphereCenter::Point(before), 4.0)
        );
        // Blocks of cells have nothing to follow.
        let block = cells([2, 2, 2], true);
        assert_eq!(block.remap(&applied.ids, None), block);
        assert_eq!(block.fixed(&sample.derived.orbits), block);
    }
}
