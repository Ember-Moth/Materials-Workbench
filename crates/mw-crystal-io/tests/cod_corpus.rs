//! Batch import of a local copy of the Crystallography Open Database. The
//! corpus is not part of the repository, so the test is ignored by default:
//!
//! ```text
//! MW_COD_DIR=~/cod/cif cargo test -p mw-crystal-io --release --test cod_corpus -- --ignored --nocapture
//! ```
//!
//! `MW_COD_LIMIT=n` stops after the first `n` files (in path order). The test
//! prints how many files yield no structure and how often each diagnostic
//! occurs, with an example file for each; it fails only if the importer
//! panics.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use mw_crystal_io::{Severity, cif};

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
struct CodeStats {
    severity: Option<Severity>,
    count: usize,
    files: usize,
    example: Option<PathBuf>,
}

#[test]
#[ignore = "needs a local COD corpus in MW_COD_DIR"]
fn cod_corpus_statistics() {
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
    let mut unreadable = 0;
    let mut without_structure = 0;
    let mut blocks = 0;
    let mut structures = 0;
    let mut failed_blocks = 0;
    let mut panics = Vec::new();
    let mut codes: BTreeMap<&'static str, CodeStats> = BTreeMap::new();

    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            unreadable += 1;
            continue;
        };
        let Ok(report) = std::panic::catch_unwind(|| cif::read(&bytes)) else {
            panics.push(path.clone());
            continue;
        };
        blocks += report.blocks.len();
        let imported = report.structures().count();
        structures += imported;
        failed_blocks += report.failed_blocks().count();
        if imported == 0 {
            without_structure += 1;
        }
        let mut seen_in_file = std::collections::HashSet::new();
        for diagnostic in report.all_diagnostics() {
            let stats = codes.entry(diagnostic.code()).or_default();
            stats.severity = Some(diagnostic.severity());
            stats.count += 1;
            if seen_in_file.insert(diagnostic.code()) {
                stats.files += 1;
            }
            stats.example.get_or_insert_with(|| path.clone());
        }
    }

    let total = files.len();
    let percent = |n: usize| 100.0 * n as f64 / total as f64;
    println!();
    println!("COD corpus: {}", dir.display());
    println!(
        "{total} files in {:.1} s; {blocks} data blocks, {structures} structures",
        start.elapsed().as_secs_f64()
    );
    println!(
        "files without a structure: {without_structure} ({:.2} %); blocks with errors: {failed_blocks}; unreadable files: {unreadable}",
        percent(without_structure)
    );
    println!();
    println!(
        "{:<8} {:<34} {:>9} {:>9} {:>8}  example",
        "severity", "code", "count", "files", "% files"
    );
    let mut rows: Vec<_> = codes.into_iter().collect();
    rows.sort_by_key(|(code, stats)| (stats.severity, std::cmp::Reverse(stats.files), *code));
    for (code, stats) in rows {
        println!(
            "{:<8} {:<34} {:>9} {:>9} {:>7.2}%  {}",
            format!("{:?}", stats.severity.expect("set with the count")),
            code,
            stats.count,
            stats.files,
            percent(stats.files),
            stats
                .example
                .as_deref()
                .and_then(|p| p.strip_prefix(&dir).ok())
                .map_or_else(String::new, |p| p.display().to_string()),
        );
    }
    assert!(panics.is_empty(), "the importer panicked on: {panics:#?}");
}
