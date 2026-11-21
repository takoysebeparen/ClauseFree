//! Typed game definitions built from files in the [`vfs::Vfs`].
//!
//! Every loader reports what it could not understand (see [`Report`]), so coverage of the game's data is
//! measurable. Loaders never panic on malformed data.
//!
//! ```no_run
//! let layers = vfs::discover_game_layers(std::path::Path::new("/path/to/game"));
//! let vfs = vfs::Vfs::new(layers);
//! let (db, report) = data::Database::load(&vfs, &data::LoadOptions::default());
//! println!("{} states, {} issues", db.states.len(), report.issues.len());
//! ```

pub mod buildings;
pub mod countries;
mod database;
pub mod defines;
pub mod equipment;
pub mod localisation;
pub mod lua;
pub mod modules;
pub mod oob;
pub mod provinces;
pub mod report;
pub mod resources;
pub mod source;
pub mod states;
pub mod technologies;
pub mod terrain;
pub mod types;
pub mod units;

pub use database::{Database, LoadOptions};
pub use report::{Issue, IssueKind, Report};
pub use types::{Color, Date, Tag};
