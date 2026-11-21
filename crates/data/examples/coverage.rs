//! Loads the database from an installation and prints what was loaded and what was not understood.
//!
//! ```text
//! cargo run --release -p data --example coverage -- [--language russian] [--all] [<game directory>]
//! ```
//!
//! Without a directory the `HOI4_DIR` environment variable is used.

use std::time::Instant;

use data::{Database, LoadOptions};

fn main() {
    let mut language = "english".to_string();
    let mut show_all = false;
    let mut dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--language" => language = args.next().unwrap_or(language),
            "--all" => show_all = true,
            _ => dir = Some(arg),
        }
    }
    let Some(dir) = dir.or_else(|| std::env::var("HOI4_DIR").ok()) else {
        eprintln!("usage: coverage [--language L] [--all] <game directory>   (or set HOI4_DIR)");
        std::process::exit(2);
    };

    let started = Instant::now();
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(std::path::Path::new(&dir)));
    let scanned = started.elapsed();
    let (db, report) = Database::load(&vfs, &LoadOptions { language });
    let loaded = started.elapsed() - scanned;

    println!("VFS: {} files in {} layers ({scanned:.2?})", vfs.len(), vfs.layers().len());
    println!("Loaded in {loaded:.2?}:");
    println!("  provinces     {}", db.provinces.len());
    println!("  terrain       {}", db.terrain.categories.len());
    println!("  countries     {}", db.countries.len());
    println!("  states        {}", db.states.len());
    println!("  localisation  {} keys ({})", db.localisation.len(), db.localisation.language);
    println!("  resources     {}", db.resources.len());
    println!("  unit categories {}", db.units.categories.len());
    println!("  sub-units     {}", db.units.sub_units.len());
    println!("  equipment     {}", db.equipment.len());
    println!("  buildings     {} ({} spawn points)", db.buildings.len(), db.buildings.spawn_points.len());
    let divisions: usize = db.oobs.by_name.values().map(|o| o.divisions.len()).sum();
    let templates: usize = db.oobs.by_name.values().map(|o| o.templates.len()).sum();
    println!("  OOB files     {} ({templates} templates, {divisions} divisions)", db.oobs.len());
    let ndefines = db.defines.section("NDefines").map_or(0, |t| t.leaf_count());
    println!("  defines       {ndefines} values in NDefines");

    let errors = report.errors().count();
    println!("\nIssues: {} total, {errors} errors", report.issues.len());
    let limit = if show_all { usize::MAX } else { 40 };
    for (label, path, count) in report.summary().into_iter().take(limit) {
        println!("  {count:>6}  {label:<45} {path}");
    }
    if !show_all {
        println!("  (use --all for every group)");
    }
}
