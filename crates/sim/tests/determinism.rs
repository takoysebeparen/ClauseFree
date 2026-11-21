//! Reproducibility of the headless core (milestone M3).
//!
//! The systems here are test fixtures, not game mechanics: they only exist to make the world change in a way that
//! depends on the generator, the clock and the command queue.

use std::path::Path;

use data::states::StateId;
use data::{Date, Tag};
use sim::command::Outcome;
use sim::time::GameTime;
use sim::world::StateControl;
use sim::{Command, CommandKind, Issuer, Pulse, Setup, Simulation, System, World};

fn tag(s: &str) -> Tag {
    Tag::parse(s).unwrap_or_else(|| panic!("tag {s}"))
}

fn start() -> GameTime {
    GameTime::from_date(Date { year: 1936, month: 1, day: 1, hour: 12 }).unwrap_or_else(|e| panic!("{e}"))
}

/// A small world that does not need the game: 3 countries, 50 states.
fn synthetic_world(seed: u64) -> World {
    let mut world = World::empty(start(), seed);
    world.countries = vec![tag("AAA"), tag("BBB"), tag("CCC")];
    for id in 1..=50u32 {
        let owner = Some(world.countries[id as usize % 3]);
        world.states.insert(StateId(id), StateControl { owner, controller: owner });
    }
    world
}

/// Every day, a random state passes to a random country.
struct DailyShuffle;

impl System for DailyShuffle {
    fn name(&self) -> &str {
        "daily_shuffle"
    }
    fn pulse(&self) -> Pulse {
        Pulse::Daily
    }
    fn run(&self, world: &mut World) {
        if world.states.is_empty() || world.countries.is_empty() {
            return;
        }
        let state_index = world.rng.below(world.states.len() as u32) as usize;
        let country = world.countries[world.rng.below(world.countries.len() as u32) as usize];
        if let Some(control) = world.states.values_mut().nth(state_index) {
            control.controller = Some(country);
        }
    }
}

/// Draws every hour, so that the generator's position depends on every simulated hour.
struct HourlyDraw;

impl System for HourlyDraw {
    fn name(&self) -> &str {
        "hourly_draw"
    }
    fn pulse(&self) -> Pulse {
        Pulse::Hourly
    }
    fn run(&self, world: &mut World) {
        world.rng.next_u32();
    }
}

fn simulation(world: World) -> Simulation {
    let mut sim = Simulation::new(world);
    sim.add_system(Box::new(HourlyDraw));
    sim.add_system(Box::new(DailyShuffle));
    sim
}

fn set_controller(tick: GameTime, issuer: u16, state: u32, controller: &str) -> Command {
    Command {
        tick,
        issuer: Issuer(issuer),
        kind: CommandKind::SetStateController { state: StateId(state), controller: tag(controller) },
    }
}

fn submit(sim: &mut Simulation, command: Command) {
    sim.submit(command).unwrap_or_else(|e| panic!("{e}"));
}

/// The hash after 400 days of the synthetic scenario with seed 1936. It is a constant so that the same test on
/// another machine, OS or compiler proves the result is bit-identical there too. If a deliberate change to the
/// world state or the systems changes it, update it in the same commit and say why.
const GOLDEN_400_DAYS: u64 = 0xdb98_c263_dece_236b;

#[test]
fn same_seed_same_result() {
    let mut a = simulation(synthetic_world(1936));
    let mut b = simulation(synthetic_world(1936));
    a.run_days(400);
    b.run_days(400);
    assert_eq!(a.world(), b.world());
    assert_eq!(a.hash(), b.hash());
    assert_eq!(a.world().time, start().plus_hours(400 * 24));
    println!("400-day hash: {:#018x}", a.hash());
    assert_eq!(a.hash(), GOLDEN_400_DAYS, "hash differs from the recorded golden value");
}

#[test]
fn different_seed_different_result() {
    let mut a = simulation(synthetic_world(1));
    let mut b = simulation(synthetic_world(2));
    a.run_days(30);
    b.run_days(30);
    assert_ne!(a.hash(), b.hash());
}

#[test]
fn hash_tracks_every_hour() {
    let mut sim = simulation(synthetic_world(7));
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..24 * 10 {
        sim.step();
        assert!(seen.insert(sim.hash()), "hash repeated at {}", sim.world().time);
    }
}

#[test]
fn commands_execute_at_their_tick() {
    let mut sim = simulation(synthetic_world(3));
    let at = start().plus_hours(5);
    submit(&mut sim, set_controller(at, 0, 1, "CCC"));
    submit(&mut sim, set_controller(at, 0, 999, "CCC"));
    submit(&mut sim, set_controller(at, 1, 2, "ZZZ"));
    sim.run_hours(4);
    assert!(sim.log().is_empty());
    let report = sim.step();
    assert_eq!((report.time, report.commands, report.rejected), (at, 3, 2));
    assert_eq!(sim.world().states[&StateId(1)].controller, Some(tag("CCC")));
    let outcomes: Vec<bool> = sim.log().iter().map(|e| e.outcome == Outcome::Applied).collect();
    assert_eq!(outcomes, [true, false, false]);
    assert!(sim.submit(set_controller(at, 0, 1, "AAA")).is_err(), "a command for a past hour must be refused");
}

/// Two peers receive the same commands in a different order; the result is identical (deterministic lockstep).
#[test]
fn arrival_order_between_issuers_does_not_matter() {
    let t1 = start().plus_hours(30);
    let t2 = start().plus_hours(31);
    let commands = [
        set_controller(t1, 0, 1, "BBB"),
        set_controller(t1, 1, 1, "CCC"),
        set_controller(t2, 0, 4, "AAA"),
        set_controller(t1, 1, 5, "AAA"),
    ];
    let mut a = simulation(synthetic_world(9));
    let mut b = simulation(synthetic_world(9));
    for c in &commands {
        submit(&mut a, c.clone());
    }
    for i in [3, 2, 1, 0] {
        submit(&mut b, commands[i].clone());
    }
    a.run_days(5);
    b.run_days(5);
    assert_eq!(a.hash(), b.hash());
}

/// The initial world plus the command log reproduces the run.
#[test]
fn replay_from_log() {
    let mut original = simulation(synthetic_world(11));
    for day in 1..=20 {
        let state = (day * 7 % 50 + 1) as u32;
        submit(&mut original, set_controller(start().plus_hours(day * 24 + 3), (day % 2) as u16, state, "BBB"));
    }
    original.run_days(30);

    let mut replay = simulation(synthetic_world(11));
    for executed in original.log() {
        submit(&mut replay, executed.command.clone());
    }
    replay.run_days(30);
    assert_eq!(replay.hash(), original.hash());
    assert_eq!(replay.log(), original.log());
}

/// Every day, each state independently may pass to a random country: computed as a parallel stage, merged in
/// state order.
struct ParallelShuffle;

impl System for ParallelShuffle {
    fn name(&self) -> &str {
        "parallel_shuffle"
    }
    fn pulse(&self) -> Pulse {
        Pulse::Daily
    }
    fn run(&self, world: &mut World) {
        let ids: Vec<StateId> = world.states.keys().copied().collect();
        let countries = world.countries.clone();
        let changes = sim::parallel::map_with_rng(&ids, &mut world.rng, |id, rng| {
            let pick = rng.below(countries.len() as u32 * 10) as usize;
            countries.get(pick).map(|tag| (*id, *tag))
        });
        for (id, tag) in changes.into_iter().flatten() {
            if let Some(control) = world.states.get_mut(&id) {
                control.controller = Some(tag);
            }
        }
    }
}

fn parallel_run(threads: usize) -> u64 {
    let mut world = synthetic_world(5);
    for id in 51..=2000u32 {
        world.states.insert(StateId(id), StateControl { owner: None, controller: None });
    }
    let mut sim = simulation(world);
    sim.add_system(Box::new(ParallelShuffle));
    sim::parallel::with_threads(threads, || {
        sim.run_days(200);
        sim.hash()
    })
    .unwrap_or_else(|e| panic!("{e}"))
}

/// The hash of [`parallel_run`]; pinned for the same reason as [`GOLDEN_400_DAYS`].
const GOLDEN_PARALLEL_200_DAYS: u64 = 0xaa93_98fc_4510_954f;

/// The result does not depend on the number of threads.
#[test]
fn parallel_stage_is_thread_count_independent() {
    let one = parallel_run(1);
    println!("parallel 200-day hash: {one:#018x}");
    assert_eq!(one, GOLDEN_PARALLEL_200_DAYS, "hash differs from the recorded golden value");
    for threads in [2, 8] {
        assert_eq!(parallel_run(threads), one, "{threads} threads");
    }
}

/// The real game's start: needs `HOI4_DIR`, skipped otherwise.
#[test]
fn real_game_runs_reproducibly() {
    let Ok(dir) = std::env::var("HOI4_DIR") else { return };
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let (db, _report) = data::Database::load(&vfs, &data::LoadOptions::default());
    let setup = Setup { seed: 1936, start: None };
    let world = World::from_database(&db, &setup).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(world.time.to_string(), "1936.1.1.12", "start date from NDefines.NGame.START_DATE");
    assert!(world.states.len() > 900, "states: {}", world.states.len());
    assert!(world.states.values().all(|s| s.owner.is_none() || s.controller.is_some()));

    let mut a = simulation(world.clone());
    let mut b = simulation(world);
    a.run_days(90);
    b.run_days(90);
    assert_eq!(a.hash(), b.hash());
}
