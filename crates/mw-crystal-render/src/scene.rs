use bytemuck::{Pod, Zeroable};
use glam::Vec3;

/// One impostor sphere as uploaded to the GPU.
///
/// Colors are linear RGB; the render target is sRGB, so the hardware encodes
/// on write.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct SphereInstance {
    /// Center in Å (xyz) and display radius in Å (w).
    pub center_radius: [f32; 4],
    /// Linear RGB color; alpha is unused.
    pub color: [f32; 4],
}

/// One impostor ellipsoid: the points `center + a·u₁ + b·u₂ + c·u₃` with
/// |u| ≤ 1, such as an atom's displacement ellipsoid. Drawn, picked and
/// highlighted like a sphere ([`Scene::atom`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct EllipsoidInstance {
    /// Center in Å (xyz) and the longest semi-axis in Å (w), which the
    /// highlight rings go around.
    pub center_radius: [f32; 4],
    /// The three semi-axes in Å (xyz). The first's w is 1 to darken the
    /// principal ellipses, where the planes of two axes cut the surface,
    /// and 0 for a plain surface.
    pub axes: [[f32; 4]; 3],
    /// Linear RGB; alpha is unused.
    pub color: [f32; 4],
}

/// One impostor cylinder: a bond drawn from `start` to `end`, each half in
/// the color of the atom at its end.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct BondInstance {
    /// Start in Å (xyz) and cylinder radius in Å (w).
    pub start_radius: [f32; 4],
    /// End in Å (xyz); w is unused.
    pub end: [f32; 4],
    /// Linear RGB of the half at the start; alpha is unused.
    pub start_color: [f32; 4],
    /// Linear RGB of the half at the end; alpha is unused.
    pub end_color: [f32; 4],
}

/// A line of constant screen width, such as a unit-cell edge, solid or
/// dashed.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct LineInstance {
    /// Start in Å (xyz) and width in logical pixels (w).
    pub start_width: [f32; 4],
    /// End in Å (xyz) and dash length in logical pixels (w); 0 is solid.
    pub end_dash: [f32; 4],
    /// Linear RGB; alpha is unused.
    pub color: [f32; 4],
}

/// A ring drawn just outside a sphere's silhouette, marking it as hovered
/// or selected. Drawn over the scene; only nearer geometry hides it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct HighlightRing {
    /// The sphere's center in Å (xyz) and radius in Å (w).
    pub center_radius: [f32; 4],
    /// Linear RGB (xyz) and ring width in logical pixels (w).
    pub color_width: [f32; 4],
}

/// A translucent convex polyhedron in one color, such as a coordination
/// polyhedron, or a flat polygon. Drawn after the opaque scene, sorted back
/// to front, seen through and seen through again; it hides nothing and is
/// not pickable, so clicks reach the atoms behind and inside it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polyhedron {
    /// Corners in Å.
    pub vertices: Vec<[f32; 3]>,
    /// Faces as triangles of indices into `vertices`, counter-clockwise
    /// seen from outside. Either side of a face may show; a flat polygon
    /// is shaded alike from both.
    pub triangles: Vec<[u32; 3]>,
    /// Linear RGB and opacity.
    pub color: [f32; 4],
}

/// Everything drawn in one frame, apart from highlights.
///
/// The atoms are the spheres followed by the ellipsoids: a picked
/// `Pick::Sphere(i)` and the highlights name the i-th of them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub spheres: Vec<SphereInstance>,
    pub ellipsoids: Vec<EllipsoidInstance>,
    pub bonds: Vec<BondInstance>,
    pub lines: Vec<LineInstance>,
    pub polyhedra: Vec<Polyhedron>,
}

impl Scene {
    /// The `index`-th atom, a sphere or (after the spheres) an ellipsoid,
    /// as its center in Å (xyz) and the radius of a sphere around it (w).
    pub fn atom(&self, index: usize) -> Option<[f32; 4]> {
        match self.spheres.get(index) {
            Some(sphere) => Some(sphere.center_radius),
            None => self
                .ellipsoids
                .get(index - self.spheres.len())
                .map(|e| e.center_radius),
        }
    }

    /// The spheres and ellipsoids.
    pub fn atom_count(&self) -> usize {
        self.spheres.len() + self.ellipsoids.len()
    }

    /// Radius of a sphere around the origin that contains everything drawn.
    pub fn bounding_radius(&self) -> f32 {
        let point = |p: &[f32]| Vec3::from_slice(&p[..3]).length();
        let spheres = self
            .spheres
            .iter()
            .map(|s| point(&s.center_radius) + s.center_radius[3]);
        let ellipsoids = self
            .ellipsoids
            .iter()
            .map(|e| point(&e.center_radius) + e.center_radius[3]);
        let bonds = self
            .bonds
            .iter()
            .map(|b| point(&b.start_radius).max(point(&b.end)) + b.start_radius[3]);
        let lines = self
            .lines
            .iter()
            .map(|l| point(&l.start_width).max(point(&l.end_dash)));
        let corners = self
            .polyhedra
            .iter()
            .flat_map(|p| p.vertices.iter().map(|v| point(v)));
        spheres
            .chain(ellipsoids)
            .chain(bonds)
            .chain(lines)
            .chain(corners)
            .fold(0.0, f32::max)
    }

    /// Moves everything by `offset`.
    pub fn translate(&mut self, offset: Vec3) {
        let shift = |p: &mut [f32; 4]| {
            let moved = Vec3::from_slice(&p[..3]) + offset;
            p[..3].copy_from_slice(&moved.to_array());
        };
        self.spheres
            .iter_mut()
            .for_each(|s| shift(&mut s.center_radius));
        self.ellipsoids
            .iter_mut()
            .for_each(|e| shift(&mut e.center_radius));
        for bond in &mut self.bonds {
            shift(&mut bond.start_radius);
            shift(&mut bond.end);
        }
        for line in &mut self.lines {
            shift(&mut line.start_width);
            shift(&mut line.end_dash);
        }
        for corner in self.polyhedra.iter_mut().flat_map(|p| &mut p.vertices) {
            *corner = (Vec3::from_array(*corner) + offset).to_array();
        }
    }
}

// Jmol colors for Ti (#BFC2C7) and O (#FF0D0D), converted to linear RGB.
const TI_COLOR: [f32; 4] = [0.521, 0.539, 0.571, 1.0];
const O_COLOR: [f32; 4] = [1.0, 0.004, 0.004, 1.0];
const TI_RADIUS: f32 = 0.6;
const O_RADIUS: f32 = 0.45;
const BOND_RADIUS: f32 = 0.15;
const SPACING: f32 = 2.0;

/// Builds exactly `count` spheres on a simple cubic grid centered at the
/// origin, alternating two species. This is a synthetic load for viewport
/// benchmarks, not a crystal structure.
pub fn synthetic_lattice(count: usize) -> Vec<SphereInstance> {
    let n = grid_size(count);
    let offset = (n as f32 - 1.0) * SPACING * 0.5;
    let mut spheres = Vec::with_capacity(count);
    'grid: for i in 0..n {
        for j in 0..n {
            for k in 0..n {
                if spheres.len() == count {
                    break 'grid;
                }
                let p = Vec3::new(i as f32, j as f32, k as f32) * SPACING - Vec3::splat(offset);
                let (radius, color) = if (i + j + k) % 2 == 0 {
                    (TI_RADIUS, TI_COLOR)
                } else {
                    (O_RADIUS, O_COLOR)
                };
                spheres.push(SphereInstance {
                    center_radius: [p.x, p.y, p.z, radius],
                    color,
                });
            }
        }
    }
    spheres
}

/// Bonds between grid neighbors of [`synthetic_lattice`], about three per
/// sphere.
pub fn synthetic_bonds(spheres: &[SphereInstance]) -> Vec<BondInstance> {
    let n = grid_size(spheres.len());
    let mut bonds = Vec::with_capacity(spheres.len() * 3);
    for (index, a) in spheres.iter().enumerate() {
        for step in [1, n, n * n] {
            // The neighbor must be on the same grid line, not wrapped around.
            let next = index + step;
            if next >= spheres.len() || (index / step) % n == n - 1 {
                continue;
            }
            let b = &spheres[next];
            let mut start_radius = a.center_radius;
            start_radius[3] = BOND_RADIUS;
            bonds.push(BondInstance {
                start_radius,
                end: b.center_radius,
                start_color: a.color,
                end_color: b.color,
            });
        }
    }
    bonds
}

/// Translucent octahedra around every `every`-th sphere of
/// [`synthetic_lattice`], in its color, with their corners on the
/// neighboring grid points: a load like coordination polyhedra.
pub fn synthetic_polyhedra(spheres: &[SphereInstance], every: usize) -> Vec<Polyhedron> {
    // The corners along ±x, ±y, ±z; faces counter-clockwise from outside.
    const TRIANGLES: [[u32; 3]; 8] = [
        [0, 2, 4],
        [2, 1, 4],
        [1, 3, 4],
        [3, 0, 4],
        [2, 0, 5],
        [1, 2, 5],
        [3, 1, 5],
        [0, 3, 5],
    ];
    spheres
        .iter()
        .step_by(every.max(1))
        .map(|sphere| {
            let center = Vec3::from_slice(&sphere.center_radius[..3]);
            let vertices = [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z]
                .map(|axis| (center + axis * SPACING).to_array())
                .to_vec();
            let [r, g, b, _] = sphere.color;
            Polyhedron {
                vertices,
                triangles: TRIANGLES.to_vec(),
                color: [r, g, b, 0.5],
            }
        })
        .collect()
}

fn grid_size(count: usize) -> usize {
    (count as f64).cbrt().ceil().max(1.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lattice_has_exact_count() {
        for count in [1, 1_000, 10_000, 100_000] {
            assert_eq!(synthetic_lattice(count).len(), count);
        }
    }

    #[test]
    fn full_lattice_is_centered() {
        let spheres = synthetic_lattice(27);
        let sum = spheres.iter().fold(Vec3::ZERO, |acc, s| {
            acc + Vec3::from_slice(&s.center_radius[..3])
        });
        assert!(sum.length() < 1e-4, "center of mass {sum:?}");
    }

    #[test]
    fn synthetic_bonds_join_grid_neighbors() {
        // A full 3 × 3 × 3 grid has 3 × 3 × 2 bonds along each axis.
        let spheres = synthetic_lattice(27);
        let bonds = synthetic_bonds(&spheres);
        assert_eq!(bonds.len(), 54);
        for bond in &bonds {
            let a = Vec3::from_slice(&bond.start_radius[..3]);
            let b = Vec3::from_slice(&bond.end[..3]);
            assert!((a.distance(b) - SPACING).abs() < 1e-5);
        }
    }

    #[test]
    fn bounds_include_bonds_and_lines() {
        let mut scene = Scene {
            lines: vec![LineInstance {
                start_width: [0.0, 0.0, 0.0, 1.0],
                end_dash: [0.0, 3.0, 4.0, 0.0],
                color: [0.0; 4],
            }],
            ..Scene::default()
        };
        assert_eq!(scene.bounding_radius(), 5.0);
        scene.translate(Vec3::new(0.0, -3.0, -4.0));
        assert_eq!(scene.lines[0].start_width, [0.0, -3.0, -4.0, 1.0]);
        assert_eq!(scene.bounding_radius(), 5.0);
    }

    #[test]
    fn atoms_are_spheres_then_ellipsoids() {
        let sphere = SphereInstance {
            center_radius: [1.0, 0.0, 0.0, 0.5],
            color: [0.0; 4],
        };
        let ellipsoid = EllipsoidInstance {
            center_radius: [0.0, 4.0, 0.0, 2.0],
            axes: [
                [2.0, 0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 0.5, 0.0],
            ],
            color: [0.0; 4],
        };
        let mut scene = Scene {
            spheres: vec![sphere],
            ellipsoids: vec![ellipsoid],
            ..Scene::default()
        };
        assert_eq!(scene.atom_count(), 2);
        assert_eq!(scene.atom(0), Some(sphere.center_radius));
        assert_eq!(scene.atom(1), Some(ellipsoid.center_radius));
        assert_eq!(scene.atom(2), None);
        assert_eq!(scene.bounding_radius(), 6.0);
        scene.translate(Vec3::new(0.0, -4.0, 0.0));
        assert_eq!(scene.atom(1), Some([0.0, 0.0, 0.0, 2.0]));
    }

    #[test]
    fn bounds_include_polyhedra() {
        let mut scene = Scene {
            polyhedra: vec![Polyhedron {
                vertices: vec![[0.0; 3], [6.0, 0.0, 8.0], [6.0, 0.0, 0.0]],
                triangles: vec![[0, 1, 2]],
                color: [1.0, 0.0, 0.0, 0.5],
            }],
            ..Scene::default()
        };
        assert_eq!(scene.bounding_radius(), 10.0);
        scene.translate(Vec3::new(-6.0, 0.0, -8.0));
        assert_eq!(scene.polyhedra[0].vertices[1], [0.0; 3]);
        assert_eq!(scene.bounding_radius(), 10.0);
    }
}
