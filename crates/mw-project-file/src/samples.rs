//! The sample projects shipped with the application (design §11): `TiO₂
//! 相研究` (rutile and a supercell derived from it) and `钙钛矿氧化物`
//! (SrTiO₃). They live in `samples/`, built by `tests/samples.rs`, and are
//! embedded here so that the application can install writable copies.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One file of a sample project, by its path below `samples/`.
pub struct SampleFile {
    pub path: &'static str,
    pub bytes: &'static [u8],
}

macro_rules! files {
    ($($path:literal),* $(,)?) => {
        &[$(SampleFile {
            path: $path,
            bytes: include_bytes!(concat!("../samples/", $path)),
        }),*]
    };
}

/// The sample project directories, in the order they are offered.
pub const PROJECTS: [&str; 2] = ["tio2.mwproj", "perovskite.mwproj"];

/// Every file of the sample projects; `tests/samples.rs` checks the list.
pub const FILES: &[SampleFile] = files![
    "perovskite.mwproj/project.json",
    "perovskite.mwproj/sources/ae3f5246ef6b63edf6cc1d8c10082d079b9aeaca998a16e39f770fd05e817672.cif",
    "perovskite.mwproj/structures/6d2a51c0-93e4-4b7a-8f10-2c4e5a6b7d01.json",
    "perovskite.mwproj/view.json",
    "tio2.mwproj/project.json",
    "tio2.mwproj/sources/fd43e5fd5ebbc2ac19a1322e14df0453062db324079355b574677a975ef69320.cif",
    "tio2.mwproj/structures/6d2a51c0-93e4-4b7a-8f10-2c4e5a6b7c01.json",
    "tio2.mwproj/structures/6d2a51c0-93e4-4b7a-8f10-2c4e5a6b7c02.json",
    "tio2.mwproj/view.json",
];

/// Writes the sample projects into `dir`, each unless it is there already,
/// and returns their paths.
pub fn install(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut installed = Vec::new();
    for project in PROJECTS {
        let target = dir.join(project);
        if !target.exists() {
            // Written next to the target and moved into place, so that an
            // interrupted install leaves no half project behind.
            let partial = dir.join(format!(".{project}.partial"));
            let _ = fs::remove_dir_all(&partial);
            for file in FILES {
                let Some(relative) = file.path.strip_prefix(project) else {
                    continue;
                };
                let path = partial.join(relative.trim_start_matches('/'));
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(path, file.bytes)?;
            }
            fs::rename(&partial, &target)?;
        }
        installed.push(target);
    }
    Ok(installed)
}
