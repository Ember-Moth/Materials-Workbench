//! Batch import of a local copy of the Crystallography Open Database. The
//! corpus is not part of the repository, so the tests are ignored by default:
//!
//! ```text
//! MW_COD_DIR=~/cod/cif cargo test -p mw-crystal-io --release --test cod_corpus -- --ignored --nocapture
//! ```
//!
//! `MW_COD_LIMIT=n` stops after the first `n` files (in path order). Files
//! are read on all cores. `MW_COD_LIST=code` also lists up to 1000 files
//! with that diagnostic code.
//!
//! - `cod_corpus_statistics`: how many files yield no structure and how
//!   often each diagnostic occurs, with its most common details and an
//!   example file; fails only if the importer panics.
//! - `cod_position_tolerance`: what snapping and merging do at several
//!   position tolerances (design §4.6).
//! - `cod_short_contacts`: distances between fully occupied sites below
//!   1 Å, to place the short-contact warning.
//! - `cod_symprec`: writes every `MW_COD_SAMPLE`-th structure (default
//!   20) out in P1 with coordinates rounded as files round them, and checks
//!   whether the symmetry search finds its operations again at several
//!   `symprec` values.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use mw_crystal::{
    Site, SiteId, SpaceGroupType, Structure, SymOp, Symmetry, Tolerances, detect_symmetry,
};
use mw_crystal_io::{DiagnosticKind, ImportReport, Severity, cif};

struct Corpus {
    dir: PathBuf,
    files: Vec<PathBuf>,
}

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

fn env_number(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|n| n.parse().ok())
}

fn corpus() -> Corpus {
    let dir = PathBuf::from(
        std::env::var_os("MW_COD_DIR").expect("set MW_COD_DIR to a directory of CIF files"),
    );
    let mut files = Vec::new();
    cif_files(&dir, &mut files);
    files.sort();
    files.truncate(env_number("MW_COD_LIMIT").unwrap_or(usize::MAX));
    assert!(!files.is_empty(), "no .cif files under {}", dir.display());
    Corpus { dir, files }
}

impl Corpus {
    fn relative<'a>(&self, path: &'a Path) -> &'a Path {
        path.strip_prefix(&self.dir).unwrap_or(path)
    }

    /// Runs `visit` on every file, on all cores. Each thread folds into its
    /// own state; the states come back for merging.
    fn visit<S: Default + Send>(&self, visit: impl Fn(&mut S, usize, &Path) + Sync) -> Vec<S> {
        let next = AtomicUsize::new(0);
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        let mut state = S::default();
                        loop {
                            let i = next.fetch_add(1, Ordering::Relaxed);
                            let Some(path) = self.files.get(i) else {
                                break;
                            };
                            visit(&mut state, i, path);
                        }
                        state
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|w| w.join().expect("worker"))
                .collect()
        })
    }
}

/// Reads and imports one file; `None` if it cannot be read or the importer
/// panics.
fn import(path: &Path, tolerances: Tolerances) -> Result<ImportReport, &'static str> {
    let bytes = std::fs::read(path).map_err(|_| "unreadable")?;
    std::panic::catch_unwind(AssertUnwindSafe(|| cif::read_with(&bytes, tolerances)))
        .map_err(|_| "panic")
}

/// A logarithmic bin for a distance or deviation, e.g. `0.003–0.01`.
fn bin(x: f64) -> String {
    const EDGES: [f64; 10] = [1e-4, 1e-3, 3e-3, 0.01, 0.03, 0.1, 0.3, 1.0, 3.0, 10.0];
    match EDGES.iter().position(|&edge| x < edge) {
        Some(0) => "< 0.0001".into(),
        Some(i) => format!("{}–{}", EDGES[i - 1], EDGES[i]),
        None => "≥ 10".into(),
    }
}

fn truncate(text: &str) -> String {
    let text = text.replace('\n', "⏎");
    match text.char_indices().nth(40) {
        Some((i, _)) => format!("{}…", &text[..i]),
        None => text,
    }
}

/// What tells occurrences of one code apart, without labels: the tag, the
/// offending text, a bin of the distance.
fn detail(kind: &DiagnosticKind) -> String {
    use DiagnosticKind::*;
    match kind {
        MissingValue { tag }
        | DuplicateTag { tag }
        | MissingCellParameter { tag }
        | AngleDefaulted { tag }
        | MissingAtomTag { tag }
        | MissingAtomValue { tag, .. } => tag.clone(),
        UnexpectedValue { text } | InvalidItNumber { text } => truncate(text),
        ReservedWord { word } => word.clone(),
        AliasConflict { used, ignored } => format!("{used} / {ignored}"),
        InvalidNumber { tag, text } | InvalidAdp { tag, text, .. } => {
            format!("{tag} {}", truncate(text))
        }
        InvalidSymop { text, error } => format!("{error:?}: {}", truncate(text)),
        InvalidSymmetry { error, .. } => format!("{error:?}")
            .split('(')
            .next()
            .unwrap_or_default()
            .to_string(),
        MissingSymops { name } => format!("{name:?}"),
        SymopsFromName { name } => truncate(name),
        SpaceGroupMismatch {
            file_symbol,
            file_number,
            number,
            ..
        } => format!("{file_symbol:?} {file_number:?} → {number}"),
        UnknownElement { symbol, .. } => truncate(symbol),
        InvalidOccupancy { value, .. } => format!("{value}"),
        OccupancyAboveOne { value, .. } | OccupancySumAboveOne { sum: value, .. } => {
            format!("{:.2}", value)
        }
        MultiplicityMismatch { file, expanded, .. } => format!("file {file}, expanded {expanded}"),
        CloseImages { distance, .. } | ShortContact { distance, .. } => bin(*distance),
        SnappedToSpecialPosition { displacement, .. } => bin(*displacement),
        CellInconsistent { length, angle } => format!("{} Å, {}°", bin(*length), bin(*angle)),
        LoopValueCount { tags, .. } => format!("{tags} tags"),
        _ => String::new(),
    }
}

#[derive(Default)]
struct CodeStats {
    severity: Option<Severity>,
    count: usize,
    files: usize,
    /// The first file in path order.
    example: Option<(usize, PathBuf)>,
    details: HashMap<String, usize>,
}

#[derive(Default)]
struct Totals {
    files: usize,
    failed_reads: Vec<(&'static str, PathBuf)>,
    without_structure: usize,
    /// Files without a structure because no block has atom coordinates,
    /// such as COD entries that give `?` for every coordinate.
    without_coordinates: usize,
    /// Files that fail to import, by each error code they have.
    failures: HashMap<&'static str, usize>,
    blocks: usize,
    structures: usize,
    failed_blocks: usize,
    codes: HashMap<&'static str, CodeStats>,
    busy: Duration,
    slowest: Vec<(Duration, PathBuf)>,
    listed: Vec<PathBuf>,
}

impl Totals {
    fn merge(&mut self, other: Totals) {
        self.files += other.files;
        self.failed_reads.extend(other.failed_reads);
        self.without_structure += other.without_structure;
        self.without_coordinates += other.without_coordinates;
        for (code, n) in other.failures {
            *self.failures.entry(code).or_default() += n;
        }
        self.blocks += other.blocks;
        self.structures += other.structures;
        self.failed_blocks += other.failed_blocks;
        self.busy += other.busy;
        self.slowest.extend(other.slowest);
        self.listed.extend(other.listed);
        for (code, stats) in other.codes {
            let into = self.codes.entry(code).or_default();
            into.severity = stats.severity;
            into.count += stats.count;
            into.files += stats.files;
            if let Some(example) = stats.example
                && into.example.as_ref().is_none_or(|e| example.0 < e.0)
            {
                into.example = Some(example);
            }
            for (detail, n) in stats.details {
                *into.details.entry(detail).or_default() += n;
            }
        }
    }
}

#[test]
#[ignore = "needs a local COD corpus in MW_COD_DIR"]
fn cod_corpus_statistics() {
    let corpus = corpus();
    let listed_code = std::env::var("MW_COD_LIST").ok();
    let start = Instant::now();
    let mut totals = Totals::default();
    for part in corpus.visit(|totals: &mut Totals, index, path| {
        totals.files += 1;
        let started = Instant::now();
        let report = match import(path, Tolerances::default()) {
            Ok(report) => report,
            Err(why) => {
                totals.failed_reads.push((why, path.to_path_buf()));
                return;
            }
        };
        let elapsed = started.elapsed();
        totals.busy += elapsed;
        totals.slowest.push((elapsed, path.to_path_buf()));
        totals.slowest.sort_by_key(|s| std::cmp::Reverse(s.0));
        totals.slowest.truncate(5);

        totals.blocks += report.blocks.len();
        let imported = report.structures().count();
        totals.structures += imported;
        totals.failed_blocks += report.failed_blocks().count();
        if imported == 0 {
            totals.without_structure += 1;
            let without_coordinates = |block: &mw_crystal_io::ImportedBlock| {
                block.diagnostics.iter().any(|d| {
                    matches!(
                        d.kind(),
                        DiagnosticKind::NoStructure | DiagnosticKind::MissingAtomSites
                    )
                })
            };
            if report.blocks.iter().all(without_coordinates) {
                totals.without_coordinates += 1;
            } else {
                let errors: HashSet<_> = report
                    .all_diagnostics()
                    .filter(|d| d.severity() == Severity::Error)
                    .map(|d| d.code())
                    .collect();
                for code in errors {
                    *totals.failures.entry(code).or_default() += 1;
                }
            }
        }
        let mut seen_in_file = HashSet::new();
        for diagnostic in report.all_diagnostics() {
            if listed_code.as_deref() == Some(diagnostic.code())
                && !seen_in_file.contains(diagnostic.code())
            {
                totals.listed.push(path.to_path_buf());
            }
            let stats = totals.codes.entry(diagnostic.code()).or_default();
            stats.severity = Some(diagnostic.severity());
            stats.count += 1;
            if seen_in_file.insert(diagnostic.code()) {
                stats.files += 1;
            }
            stats
                .example
                .get_or_insert_with(|| (index, path.to_path_buf()));
            *stats.details.entry(detail(diagnostic.kind())).or_default() += 1;
        }
    }) {
        totals.merge(part);
    }

    let total = totals.files;
    let percent = |n: usize| 100.0 * n as f64 / total as f64;
    println!();
    println!("COD corpus: {}", corpus.dir.display());
    println!(
        "{total} files in {:.1} s ({:.2} ms per file on one core); {} data blocks, {} structures",
        start.elapsed().as_secs_f64(),
        1e3 * totals.busy.as_secs_f64() / total as f64,
        totals.blocks,
        totals.structures,
    );
    let failures = totals.without_structure - totals.without_coordinates;
    println!(
        "files without a structure: {} ({:.2} %), of which {} have no atom coordinates and {} ({:.3} %) fail to import",
        totals.without_structure,
        percent(totals.without_structure),
        totals.without_coordinates,
        failures,
        percent(failures),
    );
    let mut failures: Vec<_> = totals.failures.iter().collect();
    failures.sort_by_key(|(code, n)| (std::cmp::Reverse(**n), **code));
    for (code, n) in failures {
        println!("  failing files with {code}: {n}");
    }
    println!(
        "blocks with errors: {}; unreadable files: {}",
        totals.failed_blocks,
        totals
            .failed_reads
            .iter()
            .filter(|(why, _)| *why == "unreadable")
            .count(),
    );
    totals.slowest.sort_by_key(|s| std::cmp::Reverse(s.0));
    for (elapsed, path) in totals.slowest.iter().take(5) {
        println!(
            "  slow: {:>8.1} ms  {}",
            elapsed.as_secs_f64() * 1e3,
            corpus.relative(path).display()
        );
    }
    println!();
    println!(
        "{:<8} {:<34} {:>9} {:>9} {:>8}  example",
        "severity", "code", "count", "files", "% files"
    );
    let mut rows: Vec<_> = totals.codes.into_iter().collect();
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
                .as_ref()
                .map_or_else(String::new, |(_, p)| corpus
                    .relative(p)
                    .display()
                    .to_string()),
        );
        let mut details: Vec<_> = stats
            .details
            .into_iter()
            .filter(|(d, _)| !d.is_empty())
            .collect();
        details.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (detail, n) in details.iter().take(6) {
            println!("{:<44}{n:>9}  {detail}", "");
        }
    }
    if let Some(code) = listed_code {
        totals.listed.sort();
        println!();
        println!("{} files with {code}:", totals.listed.len());
        for path in totals.listed.iter().take(1000) {
            println!("  {}", corpus.relative(path).display());
        }
    }
    let panics: Vec<_> = totals
        .failed_reads
        .iter()
        .filter(|(why, _)| *why == "panic")
        .map(|(_, path)| path)
        .collect();
    assert!(panics.is_empty(), "the importer panicked on: {panics:#?}");
}

const POSITION_TOLERANCES: [f64; 9] = [0.0001, 0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2];

/// The codes that depend on the position tolerance.
const POSITION_CODES: [&str; 9] = [
    "atom.snapped",
    "atom.merged_occupancy",
    "atom.redundant_site",
    "atom.shared_position",
    "atom.overlapping_sites",
    "atom.occupancy_sum_above_one",
    "atom.close_images",
    "atom.multiplicity_mismatch",
    "atom.short_contact",
];

#[derive(Default)]
struct AtTolerance {
    structures: usize,
    sites: usize,
    /// Per code: occurrences and files.
    codes: BTreeMap<&'static str, (usize, usize)>,
    /// Multiplicity mismatches where the file states fewer atoms than the
    /// operations generate (a site left off its special position) and more
    /// (a site moved onto one it does not belong to).
    fewer: usize,
    more: usize,
    /// Files whose structures differ in site count from the default.
    changed_files: usize,
}

#[test]
#[ignore = "needs a local COD corpus in MW_COD_DIR"]
fn cod_position_tolerance() {
    let corpus = corpus();
    let start = Instant::now();
    let parts = corpus.visit(|state: &mut Vec<AtTolerance>, _, path| {
        if state.is_empty() {
            state.resize_with(POSITION_TOLERANCES.len(), Default::default);
        }
        let sites_at = |tolerance: f64| {
            let mut tolerances = Tolerances::default();
            tolerances.position = tolerance;
            let report = import(path, tolerances).ok()?;
            let sites: Vec<usize> = report.structures().map(|(_, s)| s.sites.len()).collect();
            Some((report, sites))
        };
        let results: Vec<_> = POSITION_TOLERANCES.iter().map(|&t| sites_at(t)).collect();
        let default = Tolerances::default().position;
        let default_sites = match POSITION_TOLERANCES.iter().position(|&t| t == default) {
            Some(i) => results[i].as_ref().map(|(_, sites)| sites.clone()),
            None => sites_at(default).map(|(_, sites)| sites),
        };
        let Some(default_sites) = default_sites else {
            return;
        };
        for (at, result) in state.iter_mut().zip(results) {
            let Some((report, sites)) = result else {
                continue;
            };
            at.structures += sites.len();
            at.sites += sites.iter().sum::<usize>();
            at.changed_files += usize::from(sites != default_sites);
            let mut seen = HashSet::new();
            for diagnostic in report.all_diagnostics() {
                let code = diagnostic.code();
                let Some(code) = POSITION_CODES.iter().find(|c| **c == code) else {
                    continue;
                };
                let entry = at.codes.entry(code).or_default();
                entry.0 += 1;
                entry.1 += usize::from(seen.insert(code));
                if let DiagnosticKind::MultiplicityMismatch { file, expanded, .. } =
                    diagnostic.kind()
                {
                    if (*file as usize) < *expanded {
                        at.fewer += 1;
                    } else {
                        at.more += 1;
                    }
                }
            }
        }
    });
    let mut totals: Vec<AtTolerance> = Vec::new();
    totals.resize_with(POSITION_TOLERANCES.len(), Default::default);
    for part in parts {
        for (into, at) in totals.iter_mut().zip(part) {
            into.structures += at.structures;
            into.sites += at.sites;
            into.fewer += at.fewer;
            into.more += at.more;
            into.changed_files += at.changed_files;
            for (code, (n, files)) in at.codes {
                let entry = into.codes.entry(code).or_default();
                entry.0 += n;
                entry.1 += files;
            }
        }
    }

    println!();
    println!(
        "COD corpus: {}; {} files in {:.1} s",
        corpus.dir.display(),
        corpus.files.len(),
        start.elapsed().as_secs_f64()
    );
    println!("occurrences (files) per position tolerance in Å");
    print!("{:<30}", "");
    for tolerance in POSITION_TOLERANCES {
        print!("{:>16}", tolerance);
    }
    println!();
    let row = |name: &str, values: Vec<String>| {
        print!("{name:<30}");
        for value in values {
            print!("{value:>16}");
        }
        println!();
    };
    row(
        "structures",
        totals.iter().map(|t| t.structures.to_string()).collect(),
    );
    row(
        "sites",
        totals.iter().map(|t| t.sites.to_string()).collect(),
    );
    row(
        "files changed vs default",
        totals.iter().map(|t| t.changed_files.to_string()).collect(),
    );
    for code in POSITION_CODES {
        row(
            code,
            totals
                .iter()
                .map(|t| {
                    let (n, files) = t.codes.get(code).copied().unwrap_or_default();
                    format!("{n} ({files})")
                })
                .collect(),
        );
    }
    row(
        "  multiplicity: file < atoms",
        totals.iter().map(|t| t.fewer.to_string()).collect(),
    );
    row(
        "  multiplicity: file > atoms",
        totals.iter().map(|t| t.more.to_string()).collect(),
    );
}

/// A site counts as one atom for contact checks: ordered and fully
/// occupied, as in the importer's own check.
fn is_full(site: &Site) -> bool {
    site.disorder.is_none()
        && site
            .occupants
            .iter()
            .map(|o| o.occupancy.value())
            .sum::<f64>()
            >= 0.99
}

#[test]
#[ignore = "needs a local COD corpus in MW_COD_DIR"]
fn cod_short_contacts() {
    let corpus = corpus();
    // Per 0.05 Å bin below 1 Å: contacts with and without hydrogen, files.
    let parts = corpus.visit(
        |state: &mut BTreeMap<u32, (usize, usize, usize)>, _, path| {
            let Ok(report) = import(path, Tolerances::default()) else {
                return;
            };
            let mut seen = HashSet::new();
            for (_, structure) in report.structures() {
                let orbits = mw_crystal::orbits(structure);
                for contact in mw_crystal::close_contacts(structure, &orbits, 1.0) {
                    let (Some(a), Some(b)) = (structure.site(contact.a), structure.site(contact.b))
                    else {
                        continue;
                    };
                    if contact.distance < structure.tolerances.position
                        || !is_full(a)
                        || !is_full(b)
                    {
                        continue;
                    }
                    let hydrogen = [a, b]
                        .iter()
                        .any(|s| s.occupants.iter().any(|o| o.species.element == "H"));
                    let bin = (contact.distance / 0.05) as u32;
                    let entry = state.entry(bin).or_default();
                    if hydrogen {
                        entry.0 += 1;
                    } else {
                        entry.1 += 1;
                    }
                    entry.2 += usize::from(seen.insert(bin));
                }
            }
        },
    );
    let mut totals: BTreeMap<u32, (usize, usize, usize)> = BTreeMap::new();
    for part in parts {
        for (bin, (h, other, files)) in part {
            let entry = totals.entry(bin).or_default();
            entry.0 += h;
            entry.1 += other;
            entry.2 += files;
        }
    }
    println!();
    println!(
        "contacts between fully occupied ordered sites, {} files",
        corpus.files.len()
    );
    println!(
        "{:<14}{:>12}{:>12}{:>10}",
        "distance Å", "with H", "without H", "files"
    );
    for (bin, (h, other, files)) in totals {
        println!(
            "{:<14}{h:>12}{other:>12}{files:>10}",
            format!("{:.2}–{:.2}", bin as f64 * 0.05, (bin + 1) as f64 * 0.05)
        );
    }
}

const SYMPRECS: [f64; 6] = [0.0001, 0.001, 0.003, 0.01, 0.03, 0.1];

/// Decimals of the fractional coordinates in the P1 copies: four as in
/// many older files and databases, six as most programs write.
const DECIMALS: [i32; 2] = [4, 6];

/// How the symmetry search did on a structure written out in P1, compared
/// with the operations the file gives.
#[derive(Default, Clone, Copy)]
struct Recovery {
    same: usize,
    fewer: usize,
    more: usize,
    other: usize,
    failed: usize,
}

/// Atoms beyond which a sampled structure is skipped, to keep the run short.
const SYMPREC_ATOM_LIMIT: usize = 500;

#[test]
#[ignore = "needs a local COD corpus in MW_COD_DIR"]
fn cod_symprec() {
    let corpus = corpus();
    let every = env_number("MW_COD_SAMPLE").unwrap_or(20).max(1);
    let identity = SymOp::parse("x,y,z").expect("identity");
    #[derive(Default)]
    struct State {
        sampled: usize,
        recovery: Vec<Recovery>,
        failed: Vec<PathBuf>,
    }
    let parts = corpus.visit(|state: &mut State, index, path| {
        if state.recovery.is_empty() {
            state.recovery = vec![Recovery::default(); SYMPRECS.len() * DECIMALS.len()];
        }
        if index % every != 0 {
            return;
        }
        let Ok(report) = import(path, Tolerances::default()) else {
            return;
        };
        let Some((_, structure)) = report.structures().next() else {
            return;
        };
        let params = structure.lattice.params();
        if structure.symmetry.is_p1()
            || SpaceGroupType::identify(&structure.symmetry, params).is_none()
        {
            return;
        }
        let orbits = mw_crystal::orbits(structure);
        let atoms: usize = orbits.iter().map(|o| o.positions.len()).sum();
        if atoms > SYMPREC_ATOM_LIMIT {
            return;
        }
        state.sampled += 1;
        let operations = structure.symmetry.operations().len();
        let mut recoveries = state.recovery.iter_mut();
        for decimals in DECIMALS {
            let scale = 10f64.powi(decimals);
            let mut sites = Vec::new();
            for orbit in &orbits {
                let occupants = &structure.site(orbit.site).expect("orbit site").occupants;
                for position in &orbit.positions {
                    let id = SiteId(sites.len() as u32 + 1);
                    let rounded = position.map(|x| (x * scale).round() / scale);
                    sites.push(Site::new(id, rounded, occupants.clone()));
                }
            }
            let p1 = Symmetry::new(vec![identity]).expect("P1");
            let mut p1 = Structure::new(structure.lattice, p1, sites);
            let p1_orbits = mw_crystal::orbits(&p1);
            for &symprec in &SYMPRECS {
                let recovery = recoveries.next().expect("one per decimals and symprec");
                p1.tolerances.symprec = symprec;
                let found =
                    std::panic::catch_unwind(AssertUnwindSafe(|| detect_symmetry(&p1, &p1_orbits)));
                if !matches!(found, Ok(Some(_))) && symprec == Tolerances::default().symprec {
                    state.failed.push(path.to_path_buf());
                }
                match found {
                    Ok(Some(found)) if found.operations == operations => {
                        let identified = SpaceGroupType::identify(&structure.symmetry, params)
                            .expect("checked above");
                        if found.number == identified.number {
                            recovery.same += 1;
                        } else {
                            recovery.other += 1;
                        }
                    }
                    Ok(Some(found)) if found.operations < operations => recovery.fewer += 1,
                    Ok(Some(_)) => recovery.more += 1,
                    Ok(None) | Err(_) => recovery.failed += 1,
                }
            }
        }
    });
    let mut sampled = 0;
    let mut failed = Vec::new();
    let mut totals = vec![Recovery::default(); SYMPRECS.len() * DECIMALS.len()];
    for part in parts {
        sampled += part.sampled;
        failed.extend(part.failed);
        for (into, r) in totals.iter_mut().zip(part.recovery) {
            into.same += r.same;
            into.fewer += r.fewer;
            into.more += r.more;
            into.other += r.other;
            into.failed += r.failed;
        }
    }
    println!();
    println!(
        "{sampled} structures (every {every}th file, ≤ {SYMPREC_ATOM_LIMIT} atoms) written out in P1"
    );
    println!(
        "operations found compared with the file's: same type, fewer, more, as many of another type, none"
    );
    println!(
        "{:<10}{:<10}{:>10}{:>10}{:>10}{:>10}{:>10}",
        "decimals", "symprec", "same", "fewer", "more", "other", "failed"
    );
    let rows = DECIMALS
        .iter()
        .flat_map(|d| SYMPRECS.iter().map(move |s| (d, s)));
    for ((decimals, symprec), r) in rows.zip(totals) {
        println!(
            "{decimals:<10}{symprec:<10}{:>10}{:>10}{:>10}{:>10}{:>10}",
            r.same, r.fewer, r.more, r.other, r.failed
        );
    }
    failed.sort();
    failed.dedup();
    for path in failed.iter().take(20) {
        println!(
            "  no result at the default symprec: {}",
            corpus.relative(path).display()
        );
    }
}

/// Prints every diagnostic of the files in `MW_COD_FILE` (separated by
/// `:`) with its line, to look into one entry of the statistics.
#[test]
#[ignore = "needs MW_COD_FILE"]
fn cod_file() {
    let files = std::env::var_os("MW_COD_FILE").expect("set MW_COD_FILE to CIF files");
    for path in std::env::split_paths(&files) {
        let report = import(&path, Tolerances::default()).expect("readable");
        println!("{}", path.display());
        for block in &report.blocks {
            println!(
                "  data_{} (line {}): {}",
                block.name,
                block.line,
                block
                    .structure
                    .as_ref()
                    .map_or("no structure".into(), |s| format!(
                        "{} sites",
                        s.sites.len()
                    ))
            );
        }
        for diagnostic in report.all_diagnostics() {
            println!(
                "  {:>6}  {:?}",
                diagnostic.line().map_or("-".into(), |l| l.to_string()),
                diagnostic.kind()
            );
        }
    }
}
