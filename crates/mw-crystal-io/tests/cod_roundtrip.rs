//! CIF round trip over a local copy of the Crystallography Open Database:
//! every structure that imports is written out, read back and compared
//! (see `common::round_trip`). The corpus is not part of the repository,
//! so the test is ignored by default:
//!
//! ```text
//! MW_COD_DIR=~/cod/cif cargo test -p mw-crystal-io --release --test cod_roundtrip -- --ignored --nocapture
//! ```
//!
//! `MW_COD_LIMIT=n` stops after the first `n` files (in path order). The test
//! prints how many structures come back the same and, for the others, how
//! often each kind of mismatch occurs, with an example; it fails only if
//! reading or writing panics.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mw_crystal_io::cif;

fn cif_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            cif_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cif"))
        {
            files.push(path);
        }
    }
}

#[derive(Default)]
struct Failures {
    count: usize,
    example: Option<(PathBuf, String)>,
}

#[test]
#[ignore = "needs a local COD corpus in MW_COD_DIR"]
fn cod_round_trip() {
    let dir = PathBuf::from(
        std::env::var_os("MW_COD_DIR").expect("set MW_COD_DIR to a directory of CIF files"),
    );
    let limit = std::env::var("MW_COD_LIMIT")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(usize::MAX);
    let mut files = Vec::new();
    cif_files(&dir, &mut files);
    files.sort();
    files.truncate(limit);
    assert!(!files.is_empty(), "no .cif files under {}", dir.display());

    let start = Instant::now();
    let mut structures = 0;
    let mut same = 0;
    let mut panics = Vec::new();
    let mut failures: BTreeMap<&'static str, Failures> = BTreeMap::new();

    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(report) = std::panic::catch_unwind(|| cif::read(&bytes)) else {
            panics.push(path.clone());
            continue;
        };
        for (block, structure) in report.structures() {
            structures += 1;
            let Ok(result) = std::panic::catch_unwind(|| common::round_trip(block, structure))
            else {
                panics.push(path.clone());
                continue;
            };
            match result {
                Ok(_) => same += 1,
                Err(mismatch) => {
                    let entry = failures.entry(mismatch.category).or_default();
                    entry.count += 1;
                    entry
                        .example
                        .get_or_insert_with(|| (path.clone(), mismatch.detail));
                }
            }
        }
    }

    let percent = |n: usize| 100.0 * n as f64 / structures.max(1) as f64;
    println!();
    println!("COD round trip: {}", dir.display());
    println!(
        "{} files, {structures} structures in {:.1} s; {same} ({:.2} %) read back the same",
        files.len(),
        start.elapsed().as_secs_f64(),
        percent(same)
    );
    if !failures.is_empty() {
        println!();
        println!("{:<16} {:>9} {:>8}  example", "mismatch", "count", "%");
    }
    let mut rows: Vec<_> = failures.into_iter().collect();
    rows.sort_by_key(|(category, f)| (std::cmp::Reverse(f.count), *category));
    for (category, f) in rows {
        let (path, detail) = f.example.expect("set with the count");
        println!(
            "{:<16} {:>9} {:>7.2}%  {}: {}",
            category,
            f.count,
            percent(f.count),
            path.strip_prefix(&dir).unwrap_or(&path).display(),
            detail
        );
    }
    assert!(panics.is_empty(), "panicked on: {panics:#?}");
}
