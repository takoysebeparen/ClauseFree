//! Lists every key path used in a set of script files, with how often it occurs and an example value.
//! Used to design data loaders from what the files really contain instead of guessing.
//!
//! ```text
//! cargo run --release -p clausewitz --example key_inventory -- <directory> [--depth N] [--skip N]
//! ```
//!
//! `--skip N` replaces the first N levels of keys by `*` (use 1 for files of `name = { ... }` definitions, so that
//! all definitions are merged into one schema). Purely numeric keys and dates are shown as `<number>` / `<date>`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clausewitz::{Block, Value, decode, parse};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "txt") {
            out.push(path);
        }
    }
}

fn normal_key(key: &str) -> String {
    if !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) {
        "<number>".to_string()
    } else if key.split('.').count() >= 3
        && key.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        "<date>".to_string()
    } else {
        key.to_string()
    }
}

#[derive(Default)]
struct Stat {
    count: usize,
    example: String,
}

fn walk(
    block: &Block<'_>,
    prefix: &str,
    level: usize,
    max_depth: usize,
    skip: usize,
    stats: &mut BTreeMap<String, Stat>,
) {
    if level >= max_depth {
        return;
    }
    for entry in block.entries() {
        let key = if level < skip { "*".to_string() } else { normal_key(entry.key.text) };
        let path = if prefix.is_empty() { key } else { format!("{prefix}.{key}") };
        let stat = stats.entry(path.clone()).or_default();
        stat.count += 1;
        if stat.example.is_empty() {
            stat.example = match &entry.value {
                Value::Scalar(s) => s.text.chars().take(40).collect(),
                Value::Block(b) => format!("{{ {} items }}", b.items.len()),
                Value::Tagged(t, _) => format!("{} {{ ... }}", t.text),
                Value::Missing(_) => "<missing>".to_string(),
            };
        }
        if let Some(b) = entry.value.as_block() {
            walk(b, &path, level + 1, max_depth, skip, stats);
        }
    }
    let values = block.values().count();
    if values > 0 && level > 0 {
        stats.entry(format!("{prefix}.<list>")).or_default().count += values;
    }
}

fn main() {
    let mut dir = None;
    let (mut depth, mut skip) = (4usize, 0usize);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--depth" => depth = args.next().and_then(|v| v.parse().ok()).unwrap_or(depth),
            "--skip" => skip = args.next().and_then(|v| v.parse().ok()).unwrap_or(skip),
            _ => dir = Some(arg),
        }
    }
    let Some(dir) = dir else {
        eprintln!("usage: key_inventory <directory> [--depth N] [--skip N]");
        std::process::exit(2);
    };
    let mut files = Vec::new();
    collect(Path::new(&dir), &mut files);
    let mut stats = BTreeMap::new();
    for file in &files {
        let Ok(bytes) = std::fs::read(file) else { continue };
        let text = decode(&bytes);
        walk(&parse(&text).root, "", 0, depth, skip, &mut stats);
    }
    println!("{} files", files.len());
    for (path, stat) in &stats {
        println!("{:>7}  {path}  = {}", stat.count, stat.example);
    }
}
