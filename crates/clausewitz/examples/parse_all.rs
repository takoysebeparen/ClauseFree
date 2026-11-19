//! Parses every script file under a directory and reports problems.
//!
//! ```text
//! cargo run --release -p clausewitz --example parse_all -- "<game or mod directory>"
//! ```
//!
//! With no directory argument the `HOI4_DIR` environment variable is used.
//!
//! Some files shipped with the original game are genuinely malformed (an unclosed brace, a stray quote); the
//! original engine tolerates them. A *baseline* file records them so that only **new** problems fail the run:
//!
//! - `--write-baseline FILE`  record the current problems (relative path and problem count per line);
//! - `--baseline FILE`        exit with code 1 only if a file has more problems than the baseline allows.
//!
//! Without `--baseline`, any problem gives exit code 1.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use clausewitz::{DiagKind, LineIndex, decode, parse};

/// Files with a script extension that are plain text, not script.
const NON_SCRIPT_FILES: [&str; 2] = ["interface/credits.txt", "licenses.txt"];

/// Extensions that contain Paradox script.
const SCRIPT_EXTENSIONS: [&str; 6] = ["txt", "gfx", "gui", "asset", "mod", "map"];

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| SCRIPT_EXTENSIONS.contains(&e)) {
            out.push(path);
        }
    }
}

struct FileReport {
    path: PathBuf,
    bytes: usize,
    items: usize,
    problems: Vec<(DiagKind, usize, usize)>,
}

fn count_items(block: &clausewitz::Block<'_>) -> usize {
    block
        .items
        .iter()
        .map(|item| match item {
            clausewitz::Item::Entry(e) => 1 + e.value.as_block().map_or(0, count_items),
            clausewitz::Item::Value(v) => 1 + v.as_block().map_or(0, count_items),
        })
        .sum()
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

fn read_baseline(path: &str) -> BTreeMap<String, usize> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (file, count) = l.rsplit_once('\t')?;
            Some((file.to_string(), count.trim().parse().ok()?))
        })
        .collect()
}

fn main() {
    let mut positional = Vec::new();
    let (mut baseline_path, mut write_path) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--baseline" => baseline_path = args.next(),
            "--write-baseline" => write_path = args.next(),
            _ => positional.push(arg),
        }
    }
    let root = positional.into_iter().next().or_else(|| std::env::var("HOI4_DIR").ok());
    let Some(root) = root else {
        eprintln!("usage: parse_all [--baseline FILE | --write-baseline FILE] <directory>   (or set HOI4_DIR)");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.retain(|p| !NON_SCRIPT_FILES.contains(&relative(&root, p).as_str()));
    println!("{} script files under {}", files.len(), root.display());

    let started = Instant::now();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let reports: Vec<FileReport> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(path) = files.get(i) else { break };
                        let Ok(bytes) = std::fs::read(path) else {
                            continue;
                        };
                        let text = decode(&bytes);
                        let parsed = parse(&text);
                        let index = LineIndex::new(&text);
                        let problems = parsed
                            .diagnostics
                            .iter()
                            .map(|d| {
                                let (line, col) = index.locate(d.span.start);
                                (d.kind, line, col)
                            })
                            .collect();
                        local.push(FileReport {
                            path: path.clone(),
                            bytes: bytes.len(),
                            items: count_items(&parsed.root),
                            problems,
                        });
                    }
                    local
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    let elapsed = started.elapsed();

    let total_bytes: usize = reports.iter().map(|r| r.bytes).sum();
    let total_items: usize = reports.iter().map(|r| r.items).sum();
    let mut failing: Vec<&FileReport> = reports.iter().filter(|r| !r.problems.is_empty()).collect();
    failing.sort_by(|a, b| a.path.cmp(&b.path));
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for r in &failing {
        for (kind, _, _) in &r.problems {
            *by_kind.entry(kind.to_string()).or_default() += 1;
        }
    }

    println!(
        "parsed {} files, {:.1} MiB, {} items in {:.2?} on {} threads ({:.0} MiB/s)",
        reports.len(),
        total_bytes as f64 / 1_048_576.0,
        total_items,
        elapsed,
        threads,
        total_bytes as f64 / 1_048_576.0 / elapsed.as_secs_f64().max(1e-9)
    );
    println!("files with problems: {}", failing.len());
    for (kind, count) in &by_kind {
        println!("  {count:>6}  {kind}");
    }
    if let Some(path) = &write_path {
        let mut text = String::from("# Known problems in the original files: <relative path><TAB><problem count>\n");
        for r in &failing {
            text.push_str(&format!("{}\t{}\n", relative(&root, &r.path), r.problems.len()));
        }
        match std::fs::write(path, text) {
            Ok(()) => println!("baseline written to {path} ({} files)", failing.len()),
            Err(e) => eprintln!("cannot write {path}: {e}"),
        }
        return;
    }
    let baseline = baseline_path.as_deref().map(read_baseline);
    let mut regressions = 0;
    for r in &failing {
        let rel = relative(&root, &r.path);
        let allowed = baseline.as_ref().and_then(|b| b.get(&rel)).copied().unwrap_or(0);
        let (kind, line, col) = r.problems[0];
        let mark = if r.problems.len() > allowed {
            regressions += 1;
            "NEW"
        } else {
            "known"
        };
        if mark == "NEW" || baseline.is_none() {
            println!("  [{mark}] {rel} ({} problem(s)); first: {kind} at {line}:{col}", r.problems.len());
        }
    }
    if baseline.is_some() {
        println!("{} known problem file(s), {regressions} new", failing.len() - regressions);
    }
    if regressions > 0 {
        std::process::exit(1);
    }
}
