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

// Jmol colors for Ti (#BFC2C7) and O (#FF0D0D), converted to linear RGB.
const TI_COLOR: [f32; 4] = [0.521, 0.539, 0.571, 1.0];
const O_COLOR: [f32; 4] = [1.0, 0.004, 0.004, 1.0];
const TI_RADIUS: f32 = 0.6;
const O_RADIUS: f32 = 0.45;
const SPACING: f32 = 2.0;

/// Builds exactly `count` spheres on a simple cubic grid centered at the
/// origin, alternating two species. This is a synthetic load for viewport
/// benchmarks, not a crystal structure.
pub fn synthetic_lattice(count: usize) -> Vec<SphereInstance> {
    let n = (count as f64).cbrt().ceil().max(1.0) as usize;
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

/// Radius of a sphere around the origin that contains every instance.
pub(crate) fn bounding_radius(spheres: &[SphereInstance]) -> f32 {
    spheres
        .iter()
        .map(|s| Vec3::from_slice(&s.center_radius[..3]).length() + s.center_radius[3])
        .fold(0.0, f32::max)
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
}
