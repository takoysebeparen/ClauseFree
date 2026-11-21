//! Runs the real game's start headless for N days and prints the state hash.
//!
//! ```text
//! cargo run --release -p sim --example headless -- [days] [seed]
//! ```
//!
//! Needs `HOI4_DIR`. Running it twice (or on two machines) with the same arguments must print the same hash.
//! The wall-clock timings are printed for the developer only; they never reach the simulation.

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use sim::{Setup, Simulation, World};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let days: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(365);
    let seed: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1936);
    let Ok(dir) = std::env::var("HOI4_DIR") else {
        eprintln!("set HOI4_DIR to the game directory");
        return ExitCode::FAILURE;
    };

    let loading = Instant::now();
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let (db, report) = data::Database::load(&vfs, &data::LoadOptions::default());
    let world = match World::from_database(&db, &Setup { seed, start: None }) {
        Ok(world) => world,
        Err(e) => {
            eprintln!("cannot build the start: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "loaded in {:.1?}: {} countries, {} states, {} data issues",
        loading.elapsed(),
        world.countries.len(),
        world.states.len(),
        report.issues.len()
    );
    println!("start {} hash {:016x}", world.time, world.hash());

    let mut sim = Simulation::new(world);
    let running = Instant::now();
    sim.run_days(days);
    println!("after {days} days: {} hash {:016x} ({:.1?})", sim.world().time, sim.hash(), running.elapsed());
    ExitCode::SUCCESS
}
