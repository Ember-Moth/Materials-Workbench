//! The application icons in `packaging/icons/` (`docs/design/release.md`
//! §3–5) are rendered from `materials-workbench.svg`: the macOS `.icns` on
//! Apple's icon grid with a shadow, the Windows `.ico` (also the window's
//! icon, `build.rs`) and the Linux PNGs. After changing the drawing:
//!
//! ```text
//! cargo test -p workbench --test icons -- --ignored
//! ```

use std::path::{Path, PathBuf};

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};

/// The Windows icon's sizes: the shell and the title bar at 100–250 %
/// scaling, and the large tiles.
const WINDOWS_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// The Linux sizes, installed under `share/icons/hicolor/<n>x<n>/apps/`.
const LINUX_SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];

/// The macOS icon's entries: type and size in pixels (16 and 32 points and
/// up, at 1× and 2×).
const MACOS_ENTRIES: [(&[u8; 4], u32); 10] = [
    (b"icp4", 16),
    (b"ic11", 32),
    (b"icp5", 32),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic13", 256),
    (b"ic08", 256),
    (b"ic14", 512),
    (b"ic09", 512),
    (b"ic10", 1024),
];

/// Apple's grid: on a 1024-pixel canvas the icon is 824 pixels square, 100
/// from each edge, over a soft shadow a little below it.
const MACOS_SHADOW: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <filter id="blur" x="-10%" y="-10%" width="120%" height="120%">
    <feGaussianBlur stdDeviation="12"/>
  </filter>
  <rect x="100" y="112" width="824" height="824" rx="184" fill="#000000" fill-opacity="0.3" filter="url(#blur)"/>
</svg>"##;

fn icons_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/icons")
}

fn tree(svg: &str) -> Tree {
    Tree::from_str(svg, &Options::default()).expect("a valid drawing")
}

fn drawing() -> Tree {
    let svg = std::fs::read_to_string(icons_dir().join("materials-workbench.svg")).unwrap();
    tree(&svg)
}

/// The drawing filling a square of `size` pixels.
fn full(drawing: &Tree, size: u32) -> Pixmap {
    let mut pixmap = Pixmap::new(size, size).unwrap();
    let scale = size as f32 / drawing.size().width();
    resvg::render(
        drawing,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
}

/// The drawing on Apple's grid.
fn macos(drawing: &Tree, size: u32) -> Pixmap {
    let mut pixmap = Pixmap::new(size, size).unwrap();
    let scale = size as f32 / 1024.0;
    let shadow = tree(MACOS_SHADOW);
    resvg::render(
        &shadow,
        Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let inner = scale * 824.0 / drawing.size().width();
    let offset = scale * 100.0;
    let transform = Transform::from_row(inner, 0.0, 0.0, inner, offset, offset);
    resvg::render(drawing, transform, &mut pixmap.as_mut());
    pixmap
}

fn png(pixmap: &Pixmap) -> Vec<u8> {
    pixmap.encode_png().unwrap()
}

/// An `.ico` of PNG images, which Windows reads since Vista.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(0u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in images {
        // 0 stands for 256.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend([side, side, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend(data);
    }
    out
}

/// The images of an `.ico`: width and PNG data.
fn ico_images(data: &[u8]) -> Vec<(u32, &[u8])> {
    let u16_at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    assert_eq!((u16_at(0), u16_at(2)), (0, 1), "an icon file");
    (0..u16_at(4) as usize)
        .map(|n| {
            let entry = 6 + 16 * n;
            let side = match data[entry] {
                0 => 256,
                side => side as u32,
            };
            let (len, offset) = (u32_at(entry + 8) as usize, u32_at(entry + 12) as usize);
            (side, &data[offset..offset + len])
        })
        .collect()
}

/// An `.icns` of PNG images.
fn icns(images: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let body: Vec<u8> = images
        .iter()
        .flat_map(|(kind, data)| {
            let mut entry = kind.to_vec();
            entry.extend((data.len() as u32 + 8).to_be_bytes());
            entry.extend(data);
            entry
        })
        .collect();
    let mut out = b"icns".to_vec();
    out.extend((body.len() as u32 + 8).to_be_bytes());
    out.extend(body);
    out
}

/// The images of an `.icns`: type and data.
fn icns_images(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    assert_eq!(&data[..4], b"icns");
    let mut images = Vec::new();
    let mut at = 8;
    while at < data.len() {
        let kind: [u8; 4] = data[at..at + 4].try_into().unwrap();
        let len = u32::from_be_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
        images.push((kind, &data[at + 8..at + len]));
        at += len;
    }
    images
}

/// The files and what they hold, rendered now.
fn rendered() -> Vec<(PathBuf, Vec<u8>)> {
    let drawing = drawing();
    let dir = icons_dir();
    let mut files: Vec<(PathBuf, Vec<u8>)> = LINUX_SIZES
        .iter()
        .map(|&size| {
            let path = dir.join(format!("linux/{size}x{size}.png"));
            (path, png(&full(&drawing, size)))
        })
        .collect();
    let windows: Vec<(u32, Vec<u8>)> = WINDOWS_SIZES
        .iter()
        .map(|&size| (size, png(&full(&drawing, size))))
        .collect();
    files.push((dir.join("materials-workbench.ico"), ico(&windows)));
    let macos: Vec<(&[u8; 4], Vec<u8>)> = MACOS_ENTRIES
        .iter()
        .map(|&(kind, size)| (kind, png(&macos(&drawing, size))))
        .collect();
    files.push((dir.join("materials-workbench.icns"), icns(&macos)));
    files
}

/// Whether two PNG images agree up to rounding: renderers on other
/// processors may differ by a unit here and there.
fn same_image(a: &[u8], b: &[u8]) -> bool {
    let (a, b) = (
        Pixmap::decode_png(a).unwrap(),
        Pixmap::decode_png(b).unwrap(),
    );
    a.width() == b.width()
        && a.height() == b.height()
        && a.data()
            .iter()
            .zip(b.data())
            .all(|(x, y)| x.abs_diff(*y) <= 4)
}

#[test]
#[ignore = "writes the icons; run after changing the drawing"]
fn write_the_icons() {
    for (path, data) in rendered() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, data).unwrap();
    }
}

#[test]
fn the_icons_are_up_to_date() {
    let stale = "the drawing changed: cargo test -p workbench --test icons -- --ignored";
    for (path, fresh) in rendered() {
        let saved = std::fs::read(&path).unwrap_or_else(|_| panic!("{}: {stale}", path.display()));
        let name = path.file_name().unwrap().to_string_lossy();
        if name.ends_with(".ico") {
            let (saved, fresh) = (ico_images(&saved), ico_images(&fresh));
            assert_eq!(saved.len(), fresh.len(), "{name}: {stale}");
            for ((side, a), (fresh_side, b)) in saved.iter().zip(&fresh) {
                assert_eq!(side, fresh_side, "{name}: {stale}");
                assert!(same_image(a, b), "{name}, {side} px: {stale}");
            }
        } else if name.ends_with(".icns") {
            let (saved, fresh) = (icns_images(&saved), icns_images(&fresh));
            assert_eq!(saved.len(), fresh.len(), "{name}: {stale}");
            for ((kind, a), (fresh_kind, b)) in saved.iter().zip(&fresh) {
                assert_eq!(kind, fresh_kind, "{name}: {stale}");
                assert!(same_image(a, b), "{name}, {kind:?}: {stale}");
            }
        } else {
            assert!(same_image(&saved, &fresh), "{name}: {stale}");
        }
    }
}
