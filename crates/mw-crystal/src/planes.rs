//! Lattice planes (design §5.7): one plane of the family (hkl), its
//! interplanar spacing, and the polygon where it cuts the display range.
//! Which planes the view draws is view state.

use std::fmt;

use nalgebra::{Matrix3, Vector3};

use crate::{CellParams, DisplayRange, Lattice, Orbit};

/// Corners of the polygon that stands for a plane's disc in a sphere.
const DISC_CORNERS: usize = 72;

/// One plane of the family (hkl) (design §5.7): the points whose
/// fractional coordinates x satisfy `h·x + k·y + l·z = position`. It lies
/// `position` interplanar spacings d from the origin along the plane
/// normal, so whole positions are the planes of the family; indices with
/// a common factor, such as (222), are Laue indices with d of their own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LatticePlane {
    miller: [i32; 3],
    position: f64,
}

/// Why a plane cannot be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaneError {
    /// h, k and l are all zero.
    ZeroIndices,
    /// The position is not a finite number.
    InvalidPosition,
}

impl fmt::Display for PlaneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroIndices => write!(f, "the Miller indices must not all be zero"),
            Self::InvalidPosition => write!(f, "the position must be a finite number"),
        }
    }
}

impl std::error::Error for PlaneError {}

impl LatticePlane {
    pub fn new(miller: [i32; 3], position: f64) -> Result<Self, PlaneError> {
        if miller == [0; 3] {
            return Err(PlaneError::ZeroIndices);
        }
        if !position.is_finite() {
            return Err(PlaneError::InvalidPosition);
        }
        Ok(Self { miller, position })
    }

    /// (h, k, l).
    pub fn miller(&self) -> [i32; 3] {
        self.miller
    }

    /// The distance from the origin in interplanar spacings.
    pub fn position(&self) -> f64 {
        self.position
    }

    /// The same plane moved to another position.
    pub fn with_position(self, position: f64) -> Result<Self, PlaneError> {
        Self::new(self.miller, position)
    }

    /// The interplanar spacing d in Å.
    pub fn spacing(&self, params: &CellParams) -> f64 {
        params.d_spacing(self.miller)
    }

    /// The signed distance from the origin in Å: `position · d`.
    pub fn distance(&self, params: &CellParams) -> f64 {
        self.position * self.spacing(params)
    }

    /// The unit normal in Cartesian coordinates, along h·a* + k·b* + l·c*
    /// in the lattice's orientation.
    pub fn normal(&self, lattice: &Lattice) -> [f64; 3] {
        let n = reciprocal_vector(lattice, self.miller).normalize();
        [n.x, n.y, n.z]
    }

    /// The polygon where the plane cuts the display range, in Cartesian Å,
    /// counter-clockwise seen from the side the [normal](Self::normal)
    /// points to: within a block of cells, the plane's section of the
    /// parallelepiped the cells fill; within a sphere, its disc, as a
    /// polygon of many corners. `None` when the plane misses the range or
    /// only touches it, or a sphere's center is an atom the orbits do not
    /// have.
    pub fn section(
        &self,
        lattice: &Lattice,
        range: &DisplayRange,
        orbits: &[Orbit],
    ) -> Option<Vec<[f64; 3]>> {
        let normal = Vector3::from(self.normal(lattice));
        let corners = match *range {
            DisplayRange::Cells { counts, .. } => {
                let size = counts.map(f64::from);
                box_section(self.miller.map(f64::from), self.position, size)
                    .into_iter()
                    .map(|x| Vector3::from(lattice.to_cartesian(x)))
                    .collect()
            }
            DisplayRange::Sphere { radius, .. } => {
                let center = Vector3::from(lattice.to_cartesian(range.center(orbits)?));
                let offset = normal.dot(&center) - self.distance(lattice.params());
                if offset.abs() >= radius {
                    return None;
                }
                let foot = center - normal * offset;
                let r = (radius * radius - offset * offset).sqrt();
                let (u, v) = in_plane_basis(&normal);
                (0..DISC_CORNERS)
                    .map(|i| {
                        let angle = std::f64::consts::TAU * i as f64 / DISC_CORNERS as f64;
                        foot + (u * angle.cos() + v * angle.sin()) * r
                    })
                    .collect()
            }
        };
        let ordered = counter_clockwise(corners, &normal);
        (ordered.len() >= 3).then(|| ordered.iter().map(|p| [p.x, p.y, p.z]).collect())
    }
}

/// h·a* + k·b* + l·c* in Cartesian Å⁻¹: `A⁻ᵀ·h`, whose length is 1/d.
fn reciprocal_vector(lattice: &Lattice, miller: [i32; 3]) -> Vector3<f64> {
    let a = Matrix3::from(lattice.matrix());
    let inverse = a.try_inverse().unwrap_or_else(Matrix3::identity);
    inverse.transpose() * Vector3::from(miller.map(f64::from))
}

/// Where the plane `h·x = t` cuts the box `[0, size]` in fractional
/// coordinates: the corners on the plane and the points where it crosses
/// the box's edges, each once, in no order.
fn box_section(h: [f64; 3], t: f64, size: [f64; 3]) -> Vec<[f64; 3]> {
    let corner = |i: usize| [0, 1, 2].map(|axis| if i >> axis & 1 == 1 { size[axis] } else { 0.0 });
    let value = |p: [f64; 3]| h[0] * p[0] + h[1] * p[1] + h[2] * p[2] - t;
    // Rounding in h·x stays far below this for any box that is drawn.
    let scale = (0..3).map(|i| (h[i] * size[i]).abs()).sum::<f64>() + t.abs() + 1.0;
    let tolerance = 1e-12 * scale;
    let mut points: Vec<[f64; 3]> = Vec::new();
    let mut add = |p: [f64; 3]| {
        let far = |q: &[f64; 3]| (0..3).any(|i| (p[i] - q[i]).abs() > 1e-9 * (size[i] + 1.0));
        if points.iter().all(far) {
            points.push(p);
        }
    };
    for i in 0..8 {
        let p = corner(i);
        let f = value(p);
        if f.abs() <= tolerance {
            add(p);
        }
        for bit in [1, 2, 4] {
            let j = i | bit;
            if j == i {
                continue;
            }
            let q = corner(j);
            let g = value(q);
            if (f > tolerance && g < -tolerance) || (f < -tolerance && g > tolerance) {
                let s = f / (f - g);
                add([0, 1, 2].map(|axis| p[axis] + s * (q[axis] - p[axis])));
            }
        }
    }
    points
}

/// Two unit vectors that span the plane normal to `normal`, with
/// `u × v = normal`.
fn in_plane_basis(normal: &Vector3<f64>) -> (Vector3<f64>, Vector3<f64>) {
    // The axis least aligned with the normal gives a well-conditioned u.
    let axis = if normal.x.abs() <= normal.y.abs() && normal.x.abs() <= normal.z.abs() {
        Vector3::x()
    } else if normal.y.abs() <= normal.z.abs() {
        Vector3::y()
    } else {
        Vector3::z()
    };
    let u = normal.cross(&axis).normalize();
    let v = normal.cross(&u);
    (u, v)
}

/// The points of a convex polygon in counter-clockwise order around
/// `normal`.
fn counter_clockwise(points: Vec<Vector3<f64>>, normal: &Vector3<f64>) -> Vec<Vector3<f64>> {
    if points.len() < 3 {
        return points;
    }
    let center = points.iter().sum::<Vector3<f64>>() / points.len() as f64;
    let (u, v) = in_plane_basis(normal);
    let mut keyed: Vec<(f64, Vector3<f64>)> = points
        .into_iter()
        .map(|p| {
            let d = p - center;
            (d.dot(&v).atan2(d.dot(&u)), p)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    keyed.into_iter().map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AtomRef, Orientation, Rotation, SiteId, SphereCenter};

    fn cube(a: f64) -> Lattice {
        Lattice::new(CellParams::new(a, a, a, 90.0, 90.0, 90.0))
    }

    fn one_cell() -> DisplayRange {
        DisplayRange::default()
    }

    fn plane(miller: [i32; 3], position: f64) -> LatticePlane {
        LatticePlane::new(miller, position).unwrap()
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    /// Every corner lies on the plane, and consecutive edges turn left
    /// around the normal.
    fn check_polygon(corners: &[[f64; 3]], normal: [f64; 3], distance: f64) {
        for corner in corners {
            assert!((dot(*corner, normal) - distance).abs() < 1e-9, "{corner:?}");
        }
        let n = corners.len();
        for i in 0..n {
            let [a, b, c] = [corners[i], corners[(i + 1) % n], corners[(i + 2) % n]];
            let u = [0, 1, 2].map(|k| b[k] - a[k]);
            let v = [0, 1, 2].map(|k| c[k] - b[k]);
            let cross = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            assert!(dot(cross, normal) > 0.0, "turns right at {b:?}");
        }
    }

    #[test]
    fn refuses_zero_indices_and_infinite_positions() {
        assert_eq!(
            LatticePlane::new([0, 0, 0], 1.0),
            Err(PlaneError::ZeroIndices)
        );
        assert_eq!(
            LatticePlane::new([1, 0, 0], f64::NAN),
            Err(PlaneError::InvalidPosition)
        );
        assert_eq!(
            plane([1, 0, 0], 1.0).with_position(f64::INFINITY),
            Err(PlaneError::InvalidPosition)
        );
    }

    /// (111) one spacing from the origin cuts the cube in the triangle
    /// of the three axis corners next to the origin.
    #[test]
    fn the_111_plane_of_a_cube_is_a_triangle() {
        let lattice = cube(4.0);
        let p = plane([1, 1, 1], 1.0);
        let d = p.spacing(lattice.params());
        assert!((d - 4.0 / 3f64.sqrt()).abs() < 1e-12);
        let normal = p.normal(&lattice);
        assert!(close(normal, [1.0 / 3f64.sqrt(); 3]));
        let corners = p.section(&lattice, &one_cell(), &[]).unwrap();
        assert_eq!(corners.len(), 3);
        for axis in [[4.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 4.0]] {
            assert!(corners.iter().any(|c| close(*c, axis)), "{corners:?}");
        }
        check_polygon(&corners, normal, d);
    }

    /// Halfway between the corners, (111) cuts a regular hexagon.
    #[test]
    fn the_middle_111_plane_is_a_hexagon() {
        let lattice = cube(2.0);
        let p = plane([1, 1, 1], 1.5);
        let corners = p.section(&lattice, &one_cell(), &[]).unwrap();
        assert_eq!(corners.len(), 6);
        let center = [1.0; 3];
        for c in &corners {
            let r = [0, 1, 2].map(|i| c[i] - center[i]);
            assert!((dot(r, r).sqrt() - 2f64.sqrt()).abs() < 1e-9);
        }
        check_polygon(&corners, p.normal(&lattice), p.distance(lattice.params()));
    }

    /// A plane on a face of the cell gives that face once, corners not
    /// repeated; a plane through one corner only, or beyond the range,
    /// gives nothing.
    #[test]
    fn faces_corners_and_misses() {
        let lattice = cube(3.0);
        for position in [0.0, 1.0] {
            let face = plane([1, 0, 0], position)
                .section(&lattice, &one_cell(), &[])
                .unwrap();
            assert_eq!(face.len(), 4, "{face:?}");
            assert!(face.iter().all(|c| (c[0] - 3.0 * position).abs() < 1e-12));
        }
        assert_eq!(
            plane([1, 1, 1], 0.0).section(&lattice, &one_cell(), &[]),
            None
        );
        assert_eq!(
            plane([1, 0, 0], 1.5).section(&lattice, &one_cell(), &[]),
            None
        );
        assert_eq!(
            plane([1, 0, 0], -0.5).section(&lattice, &one_cell(), &[]),
            None
        );
        // (200) one spacing out is the plane x = ½.
        let half = plane([2, 0, 0], 1.0)
            .section(&lattice, &one_cell(), &[])
            .unwrap();
        assert!(half.iter().all(|c| (c[0] - 1.5).abs() < 1e-12));
    }

    /// In a block of cells the plane spans the whole block.
    #[test]
    fn blocks_of_cells_clip_to_the_block() {
        let lattice = cube(2.0);
        let range = DisplayRange::Cells {
            counts: [3, 2, 1],
            boundary: false,
        };
        let corners = plane([0, 0, 1], 0.5)
            .section(&lattice, &range, &[])
            .unwrap();
        assert_eq!(corners.len(), 4);
        for corner in [
            [0.0, 0.0, 1.0],
            [6.0, 0.0, 1.0],
            [6.0, 4.0, 1.0],
            [0.0, 4.0, 1.0],
        ] {
            assert!(corners.iter().any(|c| close(*c, corner)), "{corners:?}");
        }
        // x + y = 4 crosses the block of 3 × 2 cells but not one cell.
        let far = plane([1, 1, 0], 4.0);
        assert!(far.section(&lattice, &range, &[]).is_some());
        assert_eq!(far.section(&lattice, &one_cell(), &[]), None);
    }

    /// Skew cells: the corners stay on the plane at its distance from the
    /// origin, inside the cell.
    #[test]
    fn triclinic_sections_lie_on_the_plane() {
        let lattice = Lattice::new(CellParams::new(5.0, 6.0, 7.0, 75.0, 85.0, 100.0));
        for (miller, position) in [([1, 0, 0], 0.5), ([1, -1, 2], 1.0), ([2, 1, -1], 0.3)] {
            let p = plane(miller, position);
            let corners = p.section(&lattice, &one_cell(), &[]).unwrap();
            check_polygon(&corners, p.normal(&lattice), p.distance(lattice.params()));
            for corner in &corners {
                let x = lattice.to_fractional(*corner);
                assert!(x.iter().all(|v| (-1e-9..=1.0 + 1e-9).contains(v)), "{x:?}");
                let h = miller.map(f64::from);
                assert!((dot(h, x) - position).abs() < 1e-9);
            }
        }
    }

    /// A turned lattice turns the normal and the section with it; the
    /// spacing does not change.
    #[test]
    fn sections_follow_the_orientation() {
        let params = CellParams::new(3.0, 4.0, 5.0, 90.0, 90.0, 90.0);
        // 90° about z: a along y, b along −x.
        let rotation = Rotation::new([[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]).unwrap();
        let turned = Lattice::with_orientation(params, Orientation::new(rotation));
        let p = plane([1, 0, 0], 1.0);
        assert!(close(p.normal(&turned), [0.0, 1.0, 0.0]));
        let corners = p.section(&turned, &one_cell(), &[]).unwrap();
        assert!(corners.iter().all(|c| (c[1] - 3.0).abs() < 1e-12));
        check_polygon(&corners, [0.0, 1.0, 0.0], 3.0);
    }

    /// Within a sphere the plane is a disc around the foot of the center.
    #[test]
    fn spheres_clip_to_a_disc() {
        let lattice = cube(4.0);
        let range = DisplayRange::Sphere {
            center: SphereCenter::Point([0.5; 3]),
            radius: 3.0,
        };
        // x = 1 Å, one Å from the center at x = 2.
        let p = plane([1, 0, 0], 0.25);
        let corners = p.section(&lattice, &range, &[]).unwrap();
        assert_eq!(corners.len(), DISC_CORNERS);
        for c in &corners {
            assert!((c[0] - 1.0).abs() < 1e-12);
            let r = ((c[1] - 2.0).powi(2) + (c[2] - 2.0).powi(2)).sqrt();
            assert!((r - 8f64.sqrt()).abs() < 1e-9);
        }
        check_polygon(&corners, [1.0, 0.0, 0.0], 1.0);
        assert_eq!(plane([1, 0, 0], 1.5).section(&lattice, &range, &[]), None);
        // Around an atom the orbits do not have: nothing.
        let around = DisplayRange::Sphere {
            center: SphereCenter::Atom(AtomRef::new(SiteId(9), 0, [0; 3])),
            radius: 3.0,
        };
        assert_eq!(p.section(&lattice, &around, &[]), None);
    }
}
