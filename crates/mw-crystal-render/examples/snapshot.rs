//! Renders the synthetic lattice once and saves it as a PNG, for checking
//! the renderer's output without a window.
//!
//! cargo run -p mw-crystal-render --example snapshot -- out.png [atoms]

use mw_crystal_render::{OrbitCamera, PendingFrame, RenderRequest, Renderer, synthetic_lattice};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "snapshot.png".into());
    let atoms = args.next().map_or(Ok(1_000), |a| a.parse())?;

    let mut renderer = Renderer::new()?;
    let radius = renderer.set_spheres(&synthetic_lattice(atoms));
    let (width, height) = (1200, 800);
    let frame = renderer
        .render(&RenderRequest {
            width,
            height,
            camera: OrbitCamera::framing(radius).matrices(width as f32 / height as f32),
            clear_color: [0.91, 0.91, 0.92, 1.0],
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
