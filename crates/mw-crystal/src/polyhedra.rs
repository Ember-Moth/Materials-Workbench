//! Coordination polyhedra (design §5.6): the convex hull of the atoms
//! bonded to a center atom, which the "polyhedra" display model draws.
//!
//! Which atoms are centers is view state, chosen by element; by default the
//! cations, atoms whose ligands are all more electronegative, with at most
//! eight ligands. In that model a center in the display range keeps all its
//! ligands: those outside the range are shown too.

use std::collections::{BTreeSet, HashMap};

use crate::display::{atoms, position};
use crate::{
    AtomRef, Bond, Derived, DisplayAtom, DisplayRange, Displayed, Orbit, SiteId, Structure, bonds,
    element,
};

/// Ligands beyond which an atom gets no polyhedron. So many neighbors mean
/// bond rules that do not suit the structure rather than a coordination;
/// the hull search grows with the fourth power of the count.
pub const MAX_LIGANDS: usize = 24;
/// The most ligands a cation may have to be a center by default. Larger
/// cations, such as the A site of a perovskite (12), fill the cavities of a
/// framework whose polyhedra theirs would hide.
pub const DEFAULT_MAX_COORDINATION: usize = 8;

/// Which atoms are the centers of polyhedra (design §5.6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum PolyhedronCenters {
    /// The cations with at most [`DEFAULT_MAX_COORDINATION`] ligands, or all
    /// cations when no element has so few.
    #[default]
    Default,
    /// The atoms of these elements, as the user chose.
    Elements(BTreeSet<String>),
}

/// One element whose atoms can be centers: they have from 3 to
/// [`MAX_LIGANDS`] ligands.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CenterElement {
    pub element: String,
    /// The numbers of ligands its atoms have, ascending and distinct.
    pub coordination: Vec<usize>,
    /// Whether some atom of it is a cation: all its ligands are more
    /// electronegative.
    pub cation: bool,
    /// Whether it is a center by default.
    pub default: bool,
}

/// One atom of the unit cell with its ligands.
#[derive(Clone, Debug)]
struct Entry {
    element: String,
    /// The atoms bonded to it, as seen from the atom in the cell at the
    /// origin.
    ligands: Vec<AtomRef>,
    cation: bool,
}

/// The ligands of every atom of the unit cell: the atoms bonded to it under
/// the bond rules (design §5.3).
#[derive(Clone, Debug)]
pub struct Coordination {
    /// In the order of the sites and their orbits.
    entries: Vec<Entry>,
    index: HashMap<(SiteId, u16), usize>,
}

impl Coordination {
    pub fn new(structure: &Structure, orbits: &[Orbit], bonds: &[Bond]) -> Self {
        let mut entries = Vec::new();
        let mut index = HashMap::new();
        let mut elements: HashMap<SiteId, &str> = HashMap::new();
        for orbit in orbits {
            let Some(site) = structure.site(orbit.site) else {
                continue;
            };
            elements.insert(site.id, site.element());
            for member in 0..orbit.positions.len() {
                index.insert((site.id, member as u16), entries.len());
                entries.push(Entry {
                    element: site.element().to_string(),
                    ligands: Vec::new(),
                    cation: false,
                });
            }
        }
        for bond in bonds {
            if let Some(&i) = index.get(&(bond.a.site, bond.a.member)) {
                entries[i].ligands.push(bond.b);
            }
            // Seen from b in the cell at the origin, a lies in the cell
            // opposite b's.
            if let Some(&j) = index.get(&(bond.b.site, bond.b.member)) {
                entries[j]
                    .ligands
                    .push(bond.a.translated(bond.b.cell.map(|c| -c)));
            }
        }
        let electronegativity = |site: SiteId| {
            elements
                .get(&site)
                .and_then(|e| element::electronegativity(e))
        };
        for entry in &mut entries {
            let own = element::electronegativity(&entry.element);
            entry.cation = can_be_center(entry.ligands.len())
                && own.is_some_and(|own| {
                    entry
                        .ligands
                        .iter()
                        .all(|ligand| electronegativity(ligand.site).is_some_and(|e| e > own))
                });
        }
        Self { entries, index }
    }

    fn entry(&self, atom: AtomRef) -> Option<&Entry> {
        let &i = self.index.get(&(atom.site, atom.member))?;
        self.entries.get(i)
    }

    /// The atoms bonded to `atom`, in its cell; `None` if the unit cell
    /// has no such atom.
    pub fn ligands(&self, atom: AtomRef) -> Option<impl Iterator<Item = AtomRef> + '_> {
        let entry = self.entry(atom)?;
        Some(entry.ligands.iter().map(move |l| l.translated(atom.cell)))
    }

    /// Whether `atom` is a cation: it has from 3 to [`MAX_LIGANDS`] ligands,
    /// each more electronegative than itself.
    pub fn is_cation(&self, atom: AtomRef) -> bool {
        self.entry(atom).is_some_and(|entry| entry.cation)
    }

    /// The elements whose atoms can be centers, in the order of the sites.
    pub fn elements(&self) -> Vec<CenterElement> {
        // With the fewest ligands of a cation of the element, if any.
        let mut found: Vec<(CenterElement, Option<usize>)> = Vec::new();
        for entry in &self.entries {
            let count = entry.ligands.len();
            if !can_be_center(count) {
                continue;
            }
            let at = match found.iter().position(|(e, _)| e.element == entry.element) {
                Some(at) => at,
                None => {
                    found.push((
                        CenterElement {
                            element: entry.element.clone(),
                            coordination: Vec::new(),
                            cation: false,
                            default: false,
                        },
                        None,
                    ));
                    found.len() - 1
                }
            };
            let (element, fewest) = &mut found[at];
            if let Err(i) = element.coordination.binary_search(&count) {
                element.coordination.insert(i, count);
            }
            if entry.cation {
                element.cation = true;
                *fewest = Some(fewest.map_or(count, |f| f.min(count)));
            }
        }
        let small = |fewest: &Option<usize>| fewest.is_some_and(|f| f <= DEFAULT_MAX_COORDINATION);
        let any_small = found.iter().any(|(_, fewest)| small(fewest));
        found
            .into_iter()
            .map(|(mut element, fewest)| {
                element.default = if any_small {
                    small(&fewest)
                } else {
                    element.cation
                };
                element
            })
            .collect()
    }

    /// The elements whose atoms `choice` makes centers.
    pub fn centers(&self, choice: &PolyhedronCenters) -> BTreeSet<String> {
        match choice {
            PolyhedronCenters::Default => self
                .elements()
                .into_iter()
                .filter(|e| e.default)
                .map(|e| e.element)
                .collect(),
            PolyhedronCenters::Elements(elements) => elements.clone(),
        }
    }
}

fn can_be_center(ligands: usize) -> bool {
    (3..=MAX_LIGANDS).contains(&ligands)
}

impl Derived {
    /// The ligands of the atoms of the unit cell.
    pub fn coordination(&self, structure: &Structure) -> Coordination {
        Coordination::new(structure, &self.orbits, &self.bonds)
    }

    /// What `range` shows of the structure in the polyhedra model: the
    /// atoms, the polyhedra of the centers `centers` chooses with the
    /// ligands they need, and the bonds the polyhedra do not show.
    pub fn display_polyhedra(
        &self,
        structure: &Structure,
        range: &DisplayRange,
        centers: &PolyhedronCenters,
    ) -> Displayed {
        Displayed::with_polyhedra(structure, &self.orbits, &self.bonds, range, centers)
    }
}

/// The coordination polyhedron of one displayed atom.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Polyhedron {
    /// The center, an index into [`Displayed::atoms`].
    pub center: usize,
    /// Its ligands, indices into [`Displayed::atoms`], in the order of its
    /// bonds.
    pub ligands: Vec<usize>,
    /// The faces, each a polygon of indices into [`Displayed::atoms`],
    /// counter-clockwise seen from outside. A flat polygon (all ligands in
    /// one plane) is a single face.
    pub faces: Vec<Vec<usize>>,
    /// Å³; 0 for a flat polygon.
    pub volume: f64,
}

impl Polyhedron {
    /// Whether all ligands lie in one plane, as in square-planar or
    /// trigonal coordination: one polygon, seen from both sides.
    pub fn is_flat(&self) -> bool {
        self.faces.len() == 1
    }

    /// The faces split into triangles, counter-clockwise seen from outside.
    pub fn triangles(&self) -> impl Iterator<Item = [usize; 3]> + '_ {
        self.faces
            .iter()
            .flat_map(|face| (1..face.len() - 1).map(move |i| [face[0], face[i], face[i + 1]]))
    }

    /// The edges of the faces, each once, without the diagonals that split a
    /// polygonal face into triangles.
    pub fn edges(&self) -> Vec<[usize; 2]> {
        let mut edges = BTreeSet::new();
        for face in &self.faces {
            for (i, &a) in face.iter().enumerate() {
                let b = face[(i + 1) % face.len()];
                edges.insert([a.min(b), a.max(b)]);
            }
        }
        edges.into_iter().collect()
    }

    /// The corners: the ligands on the hull, each once, ascending.
    pub fn vertices(&self) -> Vec<usize> {
        let corners: BTreeSet<usize> = self.faces.iter().flatten().copied().collect();
        corners.into_iter().collect()
    }
}

impl Displayed {
    /// The atoms `range` shows, the coordination polyhedra of the centers
    /// `centers` chooses among them, and the bonds between the atoms that
    /// the polyhedra do not show (design §5.6).
    ///
    /// Each center gets all its ligands: those outside the range are added
    /// after the range's atoms, as corners only. Bonds of centers and of
    /// cations, drawn as polyhedra or as lone atoms, are left out.
    pub fn with_polyhedra(
        structure: &Structure,
        orbits: &[Orbit],
        bonds: &[Bond],
        range: &DisplayRange,
        centers: &PolyhedronCenters,
    ) -> Self {
        let mut atoms = atoms(structure, orbits, range);
        let coordination = Coordination::new(structure, orbits, bonds);
        let chosen = coordination.centers(centers);
        let mut index: HashMap<AtomRef, usize> = atoms
            .iter()
            .enumerate()
            .map(|(i, atom)| (atom.atom, i))
            .collect();
        let mut wanted: Vec<(usize, Vec<usize>)> = Vec::new();
        for center in 0..atoms.len() {
            if !chosen.contains(&atoms[center].element) {
                continue;
            }
            let atom = atoms[center].atom;
            let Some(ligands) = coordination.ligands(atom) else {
                continue;
            };
            let ligands: Vec<AtomRef> = ligands.collect();
            if !can_be_center(ligands.len()) {
                continue;
            }
            let mut found = Vec::with_capacity(ligands.len());
            for ligand in ligands {
                let shown = match index.get(&ligand) {
                    Some(&shown) => Some(shown),
                    None => add_atom(structure, orbits, ligand, &mut atoms).inspect(|&added| {
                        index.insert(ligand, added);
                    }),
                };
                found.extend(shown);
            }
            wanted.push((center, found));
        }

        let tolerance = structure.tolerances.position;
        let polyhedra: Vec<Polyhedron> = wanted
            .into_iter()
            .filter_map(|(center, ligands)| {
                let points: Vec<[f64; 3]> = ligands.iter().map(|&i| atoms[i].position).collect();
                let hull = convex_hull(&points, tolerance)?;
                let faces = hull
                    .faces
                    .iter()
                    .map(|face| face.iter().map(|&i| ligands[i]).collect())
                    .collect();
                Some(Polyhedron {
                    center,
                    ligands,
                    faces,
                    volume: hull.volume,
                })
            })
            .collect();

        let mut hidden: Vec<bool> = atoms
            .iter()
            .map(|atom| coordination.is_cation(atom.atom))
            .collect();
        for polyhedron in &polyhedra {
            hidden[polyhedron.center] = true;
        }
        let bonds = bonds::display_bonds(&atoms, bonds)
            .into_iter()
            .filter(|&[i, j]| !hidden[i] && !hidden[j])
            .collect();
        Self {
            atoms,
            bonds,
            polyhedra,
        }
    }
}

/// Adds an atom of the crystal that the range does not show; its index, or
/// `None` if the structure has no such atom.
fn add_atom(
    structure: &Structure,
    orbits: &[Orbit],
    atom: AtomRef,
    atoms: &mut Vec<DisplayAtom>,
) -> Option<usize> {
    let site = structure.site(atom.site)?;
    let fractional = position(orbits, atom)?;
    atoms.push(DisplayAtom {
        atom,
        element: site.element().to_string(),
        position: structure.lattice.to_cartesian(fractional),
    });
    Some(atoms.len() - 1)
}

/// The convex hull of a few points.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hull {
    /// Polygons of indices into the points, counter-clockwise seen from
    /// outside; one polygon when the points lie in a plane.
    pub faces: Vec<Vec<usize>>,
    /// Å³; 0 for a polygon.
    pub volume: f64,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    a.map(|v| v * s)
}

/// The convex hull of `points` (design §5.6): points closer than
/// `tolerance` to an earlier one are one corner, points within `tolerance`
/// of a face's plane belong to the face, and points within `tolerance` of
/// one plane make a flat polygon. `None` for fewer than three distinct
/// points, points on a line, or more than [`MAX_LIGANDS`] points.
///
/// Every plane through three points that has all points on one side is a
/// face: O(n⁴), which is nothing for the dozen ligands of a coordination.
pub(crate) fn convex_hull(points: &[[f64; 3]], tolerance: f64) -> Option<Hull> {
    let mut kept: Vec<usize> = Vec::new();
    for (i, p) in points.iter().enumerate() {
        if kept.iter().all(|&k| norm(sub(points[k], *p)) >= tolerance) {
            kept.push(i);
        }
    }
    let n = kept.len();
    if !(3..=MAX_LIGANDS).contains(&n) {
        return None;
    }
    let p = |i: usize| points[kept[i]];
    let farthest = |measure: &dyn Fn(usize) -> f64| {
        (0..n)
            .map(|i| (i, measure(i)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("at least three points")
    };

    // The extent: a line, then a plane through three of the points.
    let (b, length) = farthest(&|i| norm(sub(p(i), p(0))));
    if length < tolerance {
        return None;
    }
    let axis = scale(sub(p(b), p(0)), 1.0 / length);
    let (c, off_line) = farthest(&|i| norm(cross(axis, sub(p(i), p(0)))));
    if off_line < tolerance {
        return None;
    }
    let normal = cross(sub(p(b), p(0)), sub(p(c), p(0)));
    let normal = scale(normal, 1.0 / norm(normal));
    let (_, off_plane) = farthest(&|i| dot(normal, sub(p(i), p(0))).abs());
    if off_plane < tolerance {
        let all: Vec<usize> = (0..n).collect();
        let polygon = polygon(&p, &all, normal, tolerance);
        return (polygon.len() >= 3).then(|| Hull {
            faces: vec![polygon.into_iter().map(|i| kept[i]).collect()],
            volume: 0.0,
        });
    }

    // The planes of the faces, with the points on each.
    let mut planes: Vec<(Vec<usize>, [f64; 3])> = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            for k in j + 1..n {
                let (u, v) = (sub(p(j), p(i)), sub(p(k), p(i)));
                let m = cross(u, v);
                // Too close to a line to define a plane.
                if norm(m) <= tolerance * norm(u).max(norm(v)) {
                    continue;
                }
                let m = scale(m, 1.0 / norm(m));
                let heights: Vec<f64> = (0..n).map(|l| dot(m, sub(p(l), p(i)))).collect();
                let outward = if heights.iter().all(|&h| h <= tolerance) {
                    m
                } else if heights.iter().all(|&h| h >= -tolerance) {
                    scale(m, -1.0)
                } else {
                    continue;
                };
                let on: Vec<usize> = (0..n).filter(|&l| heights[l].abs() <= tolerance).collect();
                if !planes.iter().any(|(other, _)| *other == on) {
                    planes.push((on, outward));
                }
            }
        }
    }
    // Near the tolerance, three of a face's points may pass as a face of
    // their own.
    let contained = |on: &Vec<usize>| {
        planes
            .iter()
            .any(|(other, _)| other.len() > on.len() && on.iter().all(|i| other.contains(i)))
    };
    let faces: Vec<Vec<usize>> = planes
        .iter()
        .filter(|(on, _)| !contained(on))
        .map(|(on, outward)| polygon(&p, on, *outward, tolerance))
        .filter(|face| face.len() >= 3)
        .collect();

    let centroid = scale(
        (0..n).fold([0.0; 3], |sum, i| {
            let q = p(i);
            [sum[0] + q[0], sum[1] + q[1], sum[2] + q[2]]
        }),
        1.0 / n as f64,
    );
    let mut volume = 0.0;
    for face in &faces {
        let a = sub(p(face[0]), centroid);
        for pair in face[1..].windows(2) {
            let (b, c) = (sub(p(pair[0]), centroid), sub(p(pair[1]), centroid));
            volume += dot(a, cross(b, c)) / 6.0;
        }
    }
    Some(Hull {
        faces: faces
            .into_iter()
            .map(|face| face.into_iter().map(|i| kept[i]).collect())
            .collect(),
        volume: volume.abs(),
    })
}

/// The corners of points `on` that lie in a plane with normal `normal`,
/// counter-clockwise about it: their convex hull in the plane, without the
/// points inside it or within `tolerance` of its edges.
fn polygon(
    p: &dyn Fn(usize) -> [f64; 3],
    on: &[usize],
    normal: [f64; 3],
    tolerance: f64,
) -> Vec<usize> {
    // A basis of the plane with u × v = normal.
    let least = (0..3)
        .min_by(|&a, &b| normal[a].abs().total_cmp(&normal[b].abs()))
        .expect("three axes");
    let mut axis = [0.0; 3];
    axis[least] = 1.0;
    let u = cross(normal, axis);
    let u = scale(u, 1.0 / norm(u));
    let v = cross(normal, u);
    let mut points: Vec<([f64; 2], usize)> = on
        .iter()
        .map(|&i| ([dot(p(i), u), dot(p(i), v)], i))
        .collect();
    points.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]).then(a.0[1].total_cmp(&b.0[1])));

    // Andrew's monotone chain. The middle point of a turn is dropped unless
    // it lies more than `tolerance` to the left of the line through the
    // other two.
    let left = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        let cross = (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
        let base = ((b[0] - o[0]).powi(2) + (b[1] - o[1]).powi(2)).sqrt();
        cross > tolerance * base
    };
    let mut hull: Vec<([f64; 2], usize)> = Vec::with_capacity(points.len() + 1);
    for pass in [false, true] {
        let start = hull.len();
        let ordered: Box<dyn Iterator<Item = &([f64; 2], usize)>> = if pass {
            Box::new(points.iter().rev())
        } else {
            Box::new(points.iter())
        };
        for &point in ordered {
            while hull.len() >= start + 2
                && !left(hull[hull.len() - 2].0, hull[hull.len() - 1].0, point.0)
            {
                hull.pop();
            }
            hull.push(point);
        }
        // The last point of each chain starts the other.
        hull.pop();
    }
    hull.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{self, SampleKey};
    use crate::{CellParams, Lattice, Site, SymOp, Symmetry, atom_ref::distance};

    fn group(generators: &[&str]) -> Symmetry {
        let generators: Vec<SymOp> = generators
            .iter()
            .map(|g| SymOp::parse(g).unwrap())
            .collect();
        Symmetry::generate(&generators).unwrap()
    }

    fn cubic(a: f64, generators: &[&str], sites: Vec<Site>) -> Structure {
        Structure::new(
            Lattice::new(CellParams::new(a, a, a, 90.0, 90.0, 90.0)),
            group(generators),
            sites,
        )
    }

    /// Pm-3m.
    const PM3M: [&str; 3] = ["-y,x,z", "z,x,y", "-x,-y,-z"];

    /// The polyhedra of one cell with its boundary atoms.
    fn in_cell(structure: &Structure, centers: &PolyhedronCenters) -> Displayed {
        let derived = Derived::compute(structure);
        let shown = derived.display_polyhedra(structure, &DisplayRange::default(), centers);
        check(structure, &derived, &shown);
        shown
    }

    /// Every polyhedron is a closed convex surface around its center's
    /// bonded atoms: Euler's formula holds, each edge borders two faces,
    /// faces turn outward, and the corners are at bonding distance.
    fn check(structure: &Structure, derived: &Derived, shown: &Displayed) {
        let coordination = derived.coordination(structure);
        for polyhedron in &shown.polyhedra {
            let center = &shown.atoms[polyhedron.center];
            let ligands: BTreeSet<AtomRef> = coordination.ligands(center.atom).unwrap().collect();
            let corners: BTreeSet<AtomRef> = polyhedron
                .ligands
                .iter()
                .map(|&i| shown.atoms[i].atom)
                .collect();
            assert_eq!(ligands, corners, "{:?}", center.atom);
            for &i in &polyhedron.ligands {
                let r = derived
                    .cartesian(&structure.lattice, shown.atoms[i].atom)
                    .unwrap();
                assert!(distance(r, shown.atoms[i].position) < 1e-9);
                assert!(distance(center.position, r) < 4.0);
            }
            let edges = polyhedron.edges();
            let vertices = polyhedron.vertices();
            if polyhedron.is_flat() {
                assert_eq!(edges.len(), vertices.len());
                assert_eq!(polyhedron.volume, 0.0);
                continue;
            }
            assert_eq!(
                vertices.len() + polyhedron.faces.len(),
                edges.len() + 2,
                "Euler: {polyhedron:?}"
            );
            for [a, b] in &edges {
                let bordering = polyhedron
                    .faces
                    .iter()
                    .filter(|face| face.contains(a) && face.contains(b))
                    .count();
                assert_eq!(bordering, 2);
            }
            // Outward: the center of the ligands is behind every face.
            let mean = vertices.iter().fold([0.0; 3], |s, &i| {
                let p = shown.atoms[i].position;
                [s[0] + p[0], s[1] + p[1], s[2] + p[2]]
            });
            let mean = scale(mean, 1.0 / vertices.len() as f64);
            for [a, b, c] in polyhedron.triangles() {
                let [a, b, c] = [a, b, c].map(|i| shown.atoms[i].position);
                let normal = cross(sub(b, a), sub(c, a));
                assert!(dot(normal, sub(mean, a)) < 0.0);
            }
        }
    }

    fn polyhedra_of<'a>(shown: &'a Displayed, element: &str) -> Vec<&'a Polyhedron> {
        shown
            .polyhedra
            .iter()
            .filter(|p| shown.atoms[p.center].element == element)
            .collect()
    }

    #[test]
    fn rutile_titanium_sits_in_octahedra() {
        let sample = sample::load(SampleKey::Rutile);
        let rutile = &sample.structure;
        let elements = sample.derived.coordination(rutile).elements();
        // Ti is a sixfold cation; O, threefold, is listed but not a default.
        assert_eq!(elements.len(), 2);
        assert_eq!(
            (elements[0].element.as_str(), &elements[0].coordination[..]),
            ("Ti", &[6][..])
        );
        assert!(elements[0].cation && elements[0].default);
        assert_eq!(elements[1].coordination, [3]);
        assert!(!elements[1].cation && !elements[1].default);

        let shown = in_cell(rutile, &PolyhedronCenters::Default);
        // The eight corner Ti and the one at the body center.
        assert_eq!(shown.polyhedra.len(), 9);
        // The octahedron's corners: two apical O at x·a·√2 along [110], and
        // four equatorial ones in a rectangle across it.
        let (a, c, x) = (4.594, 2.959, 0.3048);
        let apical = x * a * 2f64.sqrt();
        let side = 2.0 * (0.5 - x) * a * 2f64.sqrt();
        let volume = side * c * 2.0 * apical / 3.0;
        for polyhedron in &shown.polyhedra {
            assert_eq!(polyhedron.vertices().len(), 6);
            assert_eq!(polyhedron.faces.len(), 8);
            assert!(polyhedron.faces.iter().all(|face| face.len() == 3));
            assert_eq!(polyhedron.triangles().count(), 8);
            assert_eq!(polyhedron.edges().len(), 12);
            assert!(
                (polyhedron.volume - volume).abs() < 1e-9,
                "{} vs {volume}",
                polyhedron.volume
            );
        }
        assert!((volume - 9.908).abs() < 1e-3, "{volume}");
        // The Ti–O bonds are the polyhedra; none is drawn as a stick.
        assert!(shown.bonds.is_empty());
    }

    #[test]
    fn polyhedra_reach_across_the_cell_boundary() {
        let sample = sample::load(SampleKey::Rutile);
        let rutile = &sample.structure;
        let plain = sample.derived.display(rutile, &DisplayRange::default());
        let shown = in_cell(rutile, &PolyhedronCenters::Default);
        // The range's atoms come first, as without polyhedra, so that
        // sphere i is the same atom in either model.
        assert_eq!(shown.atoms[..plain.atoms.len()], plain.atoms[..]);
        // The corner Ti's octahedra need O beyond the cell: added, with
        // their cells. Each corner has two apical O in the cell's (110)
        // diagonal, and four equatorial ones at z = ±1/2 in the cells
        // around.
        let added = &shown.atoms[plain.atoms.len()..];
        assert!(!added.is_empty());
        assert!(added.iter().all(|a| a.element == "O"));
        let fractional = |atom: &DisplayAtom| rutile.lattice.to_fractional(atom.position);
        assert!(added.iter().all(|a| {
            fractional(a)
                .iter()
                .any(|v| !(-1e-9..=1.0 + 1e-9).contains(v))
        }));
        let origin = shown
            .atoms
            .iter()
            .position(|a| a.atom == AtomRef::new(SiteId(1), 0, [0; 3]))
            .unwrap();
        let octahedron = shown.polyhedra.iter().find(|p| p.center == origin).unwrap();
        let cells: BTreeSet<[i32; 3]> = octahedron
            .ligands
            .iter()
            .map(|&i| shown.atoms[i].atom.cell)
            .collect();
        assert!(cells.len() > 2, "{cells:?}");
        assert!(cells.iter().any(|cell| cell.iter().any(|&v| v < 0)));
    }

    #[test]
    fn perovskite_draws_the_octahedra_not_the_cuboctahedra() {
        let sample = sample::load(SampleKey::StrontiumTitanate);
        let srtio3 = &sample.structure;
        let elements = sample.derived.coordination(srtio3).elements();
        let summary: Vec<(&str, &[usize], bool, bool)> = elements
            .iter()
            .map(|e| (e.element.as_str(), &e.coordination[..], e.cation, e.default))
            .collect();
        // Sr has twelve O, more than a default center may have.
        assert_eq!(
            summary,
            [
                ("Sr", &[12][..], true, false),
                ("Ti", &[6][..], true, true),
                ("O", &[6][..], false, false)
            ]
        );

        let shown = in_cell(srtio3, &PolyhedronCenters::Default);
        assert_eq!(shown.polyhedra.len(), 1);
        let ti = &shown.polyhedra[0];
        // A regular octahedron: (4/3)·r³ with r = a/2.
        let r: f64 = 3.905 / 2.0;
        assert!((ti.volume - 4.0 / 3.0 * r.powi(3)).abs() < 1e-9);
        assert_eq!((ti.faces.len(), ti.edges().len()), (8, 12));
        // Sr is a cation drawn as a lone atom: no Sr–O sticks either.
        assert!(shown.bonds.is_empty());

        // Chosen, the Sr are cuboctahedra: 8 triangles and 6 squares, whose
        // diagonals are no edges; a cube with its corners cut, (5/6)·a³.
        let both = PolyhedronCenters::Elements(["Sr".into(), "Ti".into()].into());
        let shown = in_cell(srtio3, &both);
        let sr = polyhedra_of(&shown, "Sr");
        assert_eq!(sr.len(), 8);
        for polyhedron in sr {
            let mut sizes: Vec<usize> = polyhedron.faces.iter().map(Vec::len).collect();
            sizes.sort_unstable();
            assert_eq!(sizes, [[3; 8].as_slice(), &[4; 6]].concat());
            assert_eq!(polyhedron.triangles().count(), 8 + 6 * 2);
            assert_eq!(polyhedron.edges().len(), 24);
            let volume = 5.0 / 6.0 * 3.905f64.powi(3);
            assert!((polyhedron.volume - volume).abs() < 1e-9);
        }
    }

    #[test]
    fn sphalerite_zinc_sits_in_tetrahedra() {
        // ZnS in F-43m: Zn at the origin, S at 1/4, 1/4, 1/4.
        let a = 5.4093;
        let zns = cubic(
            a,
            &[
                "-x,-y,z",
                "-x,y,-z",
                "z,x,y",
                "y,x,z",
                "x,y+1/2,z+1/2",
                "x+1/2,y,z+1/2",
            ],
            vec![
                Site::single(1, "Zn1", "Zn", [0.0, 0.0, 0.0]),
                Site::single(2, "S1", "S", [0.25, 0.25, 0.25]),
            ],
        );
        let derived = Derived::compute(&zns);
        let elements = derived.coordination(&zns).elements();
        assert_eq!(elements.len(), 2);
        assert!(elements[0].default && elements[0].element == "Zn");
        assert!(!elements[1].default);

        let shown = in_cell(&zns, &PolyhedronCenters::Default);
        // 8 corners and 6 face centers.
        assert_eq!(shown.polyhedra.len(), 14);
        let edge = a / 2f64.sqrt();
        for tetrahedron in &shown.polyhedra {
            assert_eq!(tetrahedron.vertices().len(), 4);
            assert_eq!((tetrahedron.faces.len(), tetrahedron.edges().len()), (4, 6));
            let volume = edge.powi(3) / (6.0 * 2f64.sqrt());
            assert!((tetrahedron.volume - volume).abs() < 1e-9);
        }
    }

    #[test]
    fn square_faces_of_a_cube_have_no_diagonals() {
        // CsCl: Cs at the cube's center among eight Cl.
        let cscl = cubic(
            4.123,
            &PM3M,
            vec![
                Site::single(1, "Cl1", "Cl", [0.0, 0.0, 0.0]),
                Site::single(2, "Cs1", "Cs", [0.5, 0.5, 0.5]),
            ],
        );
        let shown = in_cell(&cscl, &PolyhedronCenters::Default);
        assert_eq!(shown.polyhedra.len(), 1);
        let cube = &shown.polyhedra[0];
        assert_eq!(cube.faces.len(), 6);
        assert!(cube.faces.iter().all(|face| face.len() == 4));
        assert_eq!((cube.triangles().count(), cube.edges().len()), (12, 12));
        assert!((cube.volume - 4.123f64.powi(3)).abs() < 1e-9);
    }

    #[test]
    fn coplanar_ligands_make_a_flat_polygon() {
        // Square-planar PtCl₄ in P4/mmm, and a carbonate-like CO₃ triangle.
        let pt = Structure::new(
            Lattice::new(CellParams::new(8.0, 8.0, 6.0, 90.0, 90.0, 90.0)),
            group(&["-y,x,z", "-x,-y,-z", "x,-y,z"]),
            vec![
                Site::single(1, "Pt1", "Pt", [0.0, 0.0, 0.0]),
                Site::single(2, "Cl1", "Cl", [2.3 / 8.0, 0.0, 0.0]),
            ],
        );
        let shown = in_cell(&pt, &PolyhedronCenters::Default);
        // A Pt at each of the eight corners of the cell.
        assert_eq!(shown.polyhedra.len(), 8);
        for square in &shown.polyhedra {
            assert!(square.is_flat());
            assert_eq!(square.faces[0].len(), 4);
            assert_eq!((square.triangles().count(), square.edges().len()), (2, 4));
            assert_eq!(square.volume, 0.0);
        }

        let co3 = Structure::new(
            Lattice::new(CellParams::new(10.0, 10.0, 10.0, 90.0, 90.0, 90.0)),
            Symmetry::p1(),
            vec![
                Site::single(1, "C1", "C", [0.5, 0.5, 0.5]),
                Site::single(2, "O1", "O", [0.628, 0.5, 0.5]),
                Site::single(3, "O2", "O", [0.436, 0.6109, 0.5]),
                Site::single(4, "O3", "O", [0.436, 0.3891, 0.5]),
            ],
        );
        let shown = in_cell(&co3, &PolyhedronCenters::Default);
        assert_eq!(shown.polyhedra.len(), 1);
        let triangle = &shown.polyhedra[0];
        assert!(triangle.is_flat());
        assert_eq!(triangle.faces[0].len(), 3);
        assert!(shown.bonds.is_empty());
    }

    #[test]
    fn fewer_than_three_ligands_make_no_polyhedron() {
        // Linear O–C–O: C has two ligands, so its bonds stay sticks.
        let co2 = Structure::new(
            Lattice::new(CellParams::new(10.0, 10.0, 10.0, 90.0, 90.0, 90.0)),
            Symmetry::p1(),
            vec![
                Site::single(1, "C1", "C", [0.5, 0.5, 0.5]),
                Site::single(2, "O1", "O", [0.616, 0.5, 0.5]),
                Site::single(3, "O2", "O", [0.384, 0.5, 0.5]),
            ],
        );
        let derived = Derived::compute(&co2);
        assert!(derived.coordination(&co2).elements().is_empty());
        let shown = in_cell(&co2, &PolyhedronCenters::Default);
        assert!(shown.polyhedra.is_empty());
        assert_eq!(shown.bonds.len(), 2);
        // Asking for C changes nothing.
        let c = PolyhedronCenters::Elements(["C".into()].into());
        assert!(in_cell(&co2, &c).polyhedra.is_empty());
    }

    #[test]
    fn coincident_ligands_are_one_corner() {
        // A CO₄ tetrahedron with one O written twice, 0.003 Å apart.
        let d = 1.4 / 10.0 / 3f64.sqrt();
        let co4 = Structure::new(
            Lattice::new(CellParams::new(10.0, 10.0, 10.0, 90.0, 90.0, 90.0)),
            Symmetry::p1(),
            vec![
                Site::single(1, "C1", "C", [0.5, 0.5, 0.5]),
                Site::single(2, "O1", "O", [0.5 + d, 0.5 + d, 0.5 + d]),
                Site::single(3, "O2", "O", [0.5 + d, 0.5 - d, 0.5 - d]),
                Site::single(4, "O3", "O", [0.5 - d, 0.5 + d, 0.5 - d]),
                Site::single(5, "O4", "O", [0.5 - d, 0.5 - d, 0.5 + d]),
                Site::single(6, "O5", "O", [0.5 - d, 0.5 - d, 0.5003 + d]),
            ],
        );
        let shown = in_cell(&co4, &PolyhedronCenters::Default);
        assert_eq!(shown.polyhedra.len(), 1);
        let tetrahedron = &shown.polyhedra[0];
        assert_eq!(tetrahedron.ligands.len(), 5);
        assert_eq!(tetrahedron.vertices().len(), 4);
        assert_eq!(tetrahedron.faces.len(), 4);
    }

    #[test]
    fn hulls_of_degenerate_points() {
        let tolerance = 0.01;
        // Too few, coincident or collinear points have no hull.
        assert_eq!(convex_hull(&[[0.0; 3], [1.0, 0.0, 0.0]], tolerance), None);
        let line = [
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [3.0, 0.005, 0.0],
        ];
        assert_eq!(convex_hull(&line, tolerance), None);
        let same = [[0.0; 3], [0.001, 0.0, 0.0], [0.0, 0.002, 0.0]];
        assert_eq!(convex_hull(&same, tolerance), None);
        // A square with its center and a point on an edge: four corners.
        let square = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.5, 0.5, 0.004],
            [0.0, 1.0, 0.0],
            [0.5, 0.0, 0.0],
        ];
        let hull = convex_hull(&square, tolerance).unwrap();
        assert_eq!(hull.faces.len(), 1);
        let corners: BTreeSet<usize> = hull.faces[0].iter().copied().collect();
        assert_eq!(corners, [0, 1, 2, 4].into());
        // An octahedron with an extra point inside: six corners, eight faces.
        let mut octahedron: Vec<[f64; 3]> = (0..3)
            .flat_map(|axis| {
                [1.0, -1.0].map(|s| {
                    let mut p = [0.0; 3];
                    p[axis] = s;
                    p
                })
            })
            .collect();
        octahedron.push([0.1, 0.2, -0.1]);
        let hull = convex_hull(&octahedron, tolerance).unwrap();
        assert_eq!(hull.faces.len(), 8);
        assert!(hull.faces.iter().flatten().all(|&i| i < 6));
        assert!((hull.volume - 4.0 / 3.0).abs() < 1e-12);
        // Too many points to be a coordination.
        let many: Vec<[f64; 3]> = (0..=MAX_LIGANDS)
            .map(|i| {
                let t = i as f64;
                [t.cos(), t.sin(), (0.7 * t).sin()]
            })
            .collect();
        assert_eq!(convex_hull(&many, tolerance), None);
    }

    #[test]
    fn chosen_centers_include_anions() {
        // Rutile's O as centers: OTi₃ triangles, flat; the Ti off are lone
        // cations, so no sticks either.
        let rutile = sample::load(SampleKey::Rutile).structure;
        let o = PolyhedronCenters::Elements(["O".into()].into());
        let shown = in_cell(&rutile, &o);
        assert!(!shown.polyhedra.is_empty());
        assert!(shown.polyhedra.iter().all(|p| p.is_flat()));
        assert!(polyhedra_of(&shown, "Ti").is_empty());
        assert!(shown.bonds.is_empty());
        // None at all: every atom as in ball and stick, sticks between the
        // atoms that are no cations.
        let none = PolyhedronCenters::Elements(BTreeSet::new());
        let shown = in_cell(&rutile, &none);
        assert!(shown.polyhedra.is_empty());
    }

    #[test]
    fn large_cells_stay_fast() {
        // 10 × 10 × 10 rutile cells: 2331 octahedra.
        let sample = sample::load(SampleKey::Rutile);
        let range = DisplayRange::Cells {
            counts: [10, 10, 10],
            boundary: true,
        };
        let started = std::time::Instant::now();
        let shown = sample.derived.display_polyhedra(
            &sample.structure,
            &range,
            &PolyhedronCenters::Default,
        );
        let elapsed = started.elapsed();
        assert_eq!(shown.polyhedra.len(), 11 * 11 * 11 + 1000);
        eprintln!(
            "10 × 10 × 10 rutile: {} atoms, {} polyhedra in {elapsed:?}",
            shown.atoms.len(),
            shown.polyhedra.len()
        );
        assert!(elapsed.as_secs_f64() < 2.0, "{elapsed:?}");
    }
}
