//! The world state: everything that the simulation evolves, and nothing else.
//!
//! State is plain data in ordered containers (`BTreeMap`, sorted `Vec`) indexed by stable ids, so iteration order is
//! defined and the hash is reproducible. It grows system by system; for now it holds the clock, the generator and
//! the political map at the start date (who owns and who controls each state).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use data::states::StateId;
use data::{Database, Date, Tag};

use crate::hash::{StateHash, StateHasher, hash_of};
use crate::rng::Rng;
use crate::time::{GameTime, TimeError};

/// The define that holds the start date of a new game.
pub const START_DATE_DEFINE: &str = "NDefines.NGame.START_DATE";

/// Owner and controller of one state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateControl {
    pub owner: Option<Tag>,
    pub controller: Option<Tag>,
}

impl StateHash for StateControl {
    fn hash_into(&self, h: &mut StateHasher) {
        self.owner.hash_into(h);
        self.controller.hash_into(h);
    }
}

/// The simulated world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct World {
    /// The last simulated hour.
    pub time: GameTime,
    pub rng: Rng,
    /// Every country tag, sorted.
    pub countries: Vec<Tag>,
    pub states: BTreeMap<StateId, StateControl>,
}

/// How to build the starting world.
#[derive(Debug, Clone, Default)]
pub struct Setup {
    /// Seed of the world's generator.
    pub seed: u64,
    /// Start date; `None` uses the `NDefines.NGame.START_DATE` define.
    pub start: Option<Date>,
}

/// Why the starting world could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupError {
    /// No start date was given and the define is missing or not a string.
    MissingStartDate,
    /// The start date text does not parse.
    BadStartDate(String),
    /// The start date does not exist in the calendar.
    InvalidStartDate(TimeError),
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SetupError::MissingStartDate => write!(f, "no start date given and {START_DATE_DEFINE} is not set"),
            SetupError::BadStartDate(text) => write!(f, "start date {text:?} does not parse"),
            SetupError::InvalidStartDate(e) => write!(f, "start date: {e}"),
        }
    }
}

impl std::error::Error for SetupError {}

impl World {
    /// An empty world at `time`.
    pub fn empty(time: GameTime, seed: u64) -> World {
        World { time, rng: Rng::new(seed, 0), countries: Vec::new(), states: BTreeMap::new() }
    }

    /// The world at the start date, from loaded definitions.
    ///
    /// Only the undated part of state history is applied, which is the 1936 start of the base game. Dated history
    /// blocks (later bookmarks) are applied once the script layer can run effects.
    ///
    /// **Guess (see `docs/CANON.md`, "Start of a game"):** a state whose history names an owner but no controller is
    /// controlled by its owner.
    pub fn from_database(db: &Database, setup: &Setup) -> Result<World, SetupError> {
        let start = match setup.start {
            Some(date) => date,
            None => {
                let text = db.defines.string(START_DATE_DEFINE).ok_or(SetupError::MissingStartDate)?;
                Date::parse(text).ok_or_else(|| SetupError::BadStartDate(text.to_string()))?
            }
        };
        let time = GameTime::from_date(start).map_err(SetupError::InvalidStartDate)?;
        let mut world = World::empty(time, setup.seed);
        let countries: BTreeSet<Tag> = db.countries.by_tag.keys().copied().collect();
        world.countries = countries.into_iter().collect();
        for (id, def) in &db.states.by_id {
            let owner = def.history.owner;
            let controller = def.history.controller.or(owner);
            world.states.insert(*id, StateControl { owner, controller });
        }
        Ok(world)
    }

    pub fn has_country(&self, tag: Tag) -> bool {
        self.countries.binary_search(&tag).is_ok()
    }

    /// The hash of the whole world state.
    pub fn hash(&self) -> u64 {
        hash_of(self)
    }
}

impl StateHash for World {
    fn hash_into(&self, h: &mut StateHasher) {
        self.time.hash_into(h);
        self.rng.hash_into(h);
        self.countries.hash_into(h);
        self.states.hash_into(h);
    }
}
