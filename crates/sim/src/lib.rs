//! Deterministic, headless world simulation.
//!
//! The simulation advances in whole hours. Each step:
//!
//! 1. advances the clock by one hour;
//! 2. executes the commands due at that hour, in queue order (see [`command`]);
//! 3. runs the systems of every pulse that starts at that hour: hourly, then daily, monthly, yearly; within a pulse,
//!    systems run in the order they were added.
//!
//! The order of steps 2 and 3, and of the pulses, is our choice (see `docs/CANON.md`, "Clock pulses"); the
//! original's order is unmeasured.
//!
//! Rules that keep it deterministic: no floating point, no wall clock, no unordered iteration, one seeded generator
//! in the world state, and systems without hidden state (they receive `&self`, so everything they remember lives in
//! the [`World`], where it is hashed and saved).
//!
//! ```
//! use sim::{Simulation, World, time::GameTime};
//!
//! let mut sim = Simulation::new(World::empty(GameTime(0), 42));
//! sim.run_days(30);
//! println!("{} {:016x}", sim.world().time, sim.hash());
//! ```

pub mod command;
pub mod hash;
pub mod parallel;
pub mod rng;
pub mod time;
pub mod world;

pub use command::{Command, CommandKind, Issuer};
pub use world::{Setup, SetupError, World};

use command::{CommandQueue, Executed, Outcome, Rejection, SubmitError};
use time::{GameTime, HOURS_PER_DAY};

/// When a system runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pulse {
    Hourly,
    Daily,
    Monthly,
    Yearly,
}

/// A piece of simulation logic. It must keep all its state in the [`World`].
pub trait System: Send + Sync {
    /// A short name for logs and profiling.
    fn name(&self) -> &str;
    fn pulse(&self) -> Pulse;
    fn run(&self, world: &mut World);
}

/// What one step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepReport {
    pub time: GameTime,
    pub commands: usize,
    pub rejected: usize,
    pub new_day: bool,
    pub new_month: bool,
    pub new_year: bool,
}

/// The world plus the machinery that advances it.
pub struct Simulation {
    world: World,
    queue: CommandQueue,
    systems: Vec<Box<dyn System>>,
    log: Vec<Executed>,
}

impl Simulation {
    pub fn new(world: World) -> Simulation {
        Simulation { world, queue: CommandQueue::default(), systems: Vec::new(), log: Vec::new() }
    }

    /// Adds a system; systems of the same pulse run in the order they were added.
    pub fn add_system(&mut self, system: Box<dyn System>) {
        self.systems.push(system);
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// The hash of the world state.
    pub fn hash(&self) -> u64 {
        self.world.hash()
    }

    /// Every executed command with its outcome, in execution order. Initial world + this log replays the game.
    pub fn log(&self) -> &[Executed] {
        &self.log
    }

    /// Queues a command. It must be for a later hour than the current one.
    pub fn submit(&mut self, command: Command) -> Result<u64, SubmitError> {
        self.queue.submit(command, self.world.time)
    }

    /// Simulates one hour.
    pub fn step(&mut self) -> StepReport {
        self.world.time = self.world.time.plus_hours(1);
        let now = self.world.time;
        let due = self.queue.take_due(now);
        let commands = due.len();
        let mut rejected = 0;
        for (command, sequence) in due {
            let outcome = apply(&mut self.world, &command.kind);
            if outcome != Outcome::Applied {
                rejected += 1;
            }
            self.log.push(Executed { command, sequence, outcome });
        }
        let pulses = now.pulses();
        self.run_pulse(Pulse::Hourly);
        if pulses.day {
            self.run_pulse(Pulse::Daily);
        }
        if pulses.month {
            self.run_pulse(Pulse::Monthly);
        }
        if pulses.year {
            self.run_pulse(Pulse::Yearly);
        }
        StepReport {
            time: now,
            commands,
            rejected,
            new_day: pulses.day,
            new_month: pulses.month,
            new_year: pulses.year,
        }
    }

    pub fn run_hours(&mut self, hours: u64) {
        for _ in 0..hours {
            self.step();
        }
    }

    pub fn run_days(&mut self, days: u64) {
        self.run_hours(days.saturating_mul(HOURS_PER_DAY as u64));
    }

    /// Runs until the clock reaches `time` (no-op if it already has).
    pub fn run_until(&mut self, time: GameTime) {
        while self.world.time < time {
            self.step();
        }
    }

    fn run_pulse(&mut self, pulse: Pulse) {
        for system in &self.systems {
            if system.pulse() == pulse {
                system.run(&mut self.world);
            }
        }
    }
}

fn apply(world: &mut World, kind: &CommandKind) -> Outcome {
    match kind {
        CommandKind::SetStateController { state, controller } => {
            if !world.has_country(*controller) {
                return Outcome::Rejected(Rejection::UnknownCountry(*controller));
            }
            match world.states.get_mut(state) {
                Some(control) => {
                    control.controller = Some(*controller);
                    Outcome::Applied
                }
                None => Outcome::Rejected(Rejection::UnknownState(*state)),
            }
        }
    }
}
