//! Renders the synthetic lattice once and saves it as a PNG, for checking
//! the renderer's output without a window: spheres, bonds, a bounding box of
//! lines and a highlight ring on the last sphere.
//!
//! cargo run -p mw-crystal-render --example snapshot -- out.png [atoms]

use mw_crystal_render::{
    HighlightRing, LineInstance, OrbitCamera, PendingFrame, RenderRequest, Renderer, Scene,
    synthetic_bonds, synthetic_lattice,
};

/// The twelve edges of an axis-aligned box from −h to h.
fn box_edges(h: f32, color: [f32; 4]) -> Vec<LineInstance> {
    let corner = |i: usize| [0, 1, 2].map(|axis| if i >> axis & 1 == 1 { h } else { -h });
    let mut lines = Vec::new();
    for i in 0..8 {
        for axis in 0..3 {
            let j = i | 1 << axis;
            if j != i {
                let [x, y, z] = corner(i);
                let [ex, ey, ez] = corner(j);
                lines.push(LineInstance {
                    start_width: [x, y, z, 2.0],
                    end_dash: [ex, ey, ez, 0.0],
                    color,
                });
            }
        }
    }
    lines
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "snapshot.png".into());
    let atoms = args.next().map_or(Ok(1_000), |a| a.parse())?;

    let mut renderer = Renderer::new()?;
    let spheres = synthetic_lattice(atoms);
    let extent = spheres
        .iter()
        .map(|s| s.center_radius[0].abs() + s.center_radius[3])
        .fold(0.0, f32::max);
    let last = spheres.last().copied();
    let radius = renderer.set_scene(&Scene {
        bonds: synthetic_bonds(&spheres),
        lines: box_edges(extent, [0.1, 0.1, 0.1, 1.0]),
        spheres,
    });
    if let Some(last) = last {
        renderer.set_highlights(&[HighlightRing {
            center_radius: last.center_radius,
            color_width: [0.1, 0.35, 1.0, 4.0],
        }]);
    }
    let (width, height) = (1200, 800);
    let frame = renderer
        .render(&RenderRequest {
            width,
            height,
            camera: OrbitCamera::framing(radius).matrices(width as f32 / height as f32),
            clear_color: [0.91, 0.91, 0.92, 1.0],
            scale_factor: 1.0,
        })
        .and_then(PendingFrame::wait)?;

    let mut rgba = frame.bgra;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    image::RgbaImage::from_raw(frame.width, frame.height, rgba)
        .ok_or("frame size mismatch")?
        .save(&path)?;
    println!(
        "{} atoms -> {path} ({}x{})",
        atoms, frame.width, frame.height
    );
    Ok(())
}
