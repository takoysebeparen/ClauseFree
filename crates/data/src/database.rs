//! Loading everything and checking references between definitions.

use vfs::Vfs;

use crate::countries::{self, Countries};
use crate::localisation::{self, Localisation};
use crate::provinces::{self, Provinces};
use crate::report::{IssueKind, Report};
use crate::states::{self, States};
use crate::terrain::{self, Terrain};

#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// Localisation language (`english`, `russian`, ...).
    pub language: String,
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions { language: "english".to_string() }
    }
}

/// All loaded definitions.
#[derive(Debug, Clone, Default)]
pub struct Database {
    pub provinces: Provinces,
    pub terrain: Terrain,
    pub countries: Countries,
    pub states: States,
    pub localisation: Localisation,
}

impl Database {
    pub fn load(vfs: &Vfs, options: &LoadOptions) -> (Database, Report) {
        let mut report = Report::default();
        let db = Database {
            provinces: provinces::load(vfs, &mut report),
            terrain: terrain::load(vfs, &mut report),
            countries: countries::load(vfs, &mut report),
            states: states::load(vfs, &mut report),
            localisation: localisation::load(vfs, &options.language, &mut report),
        };
        db.check_references(&mut report);
        (db, report)
    }

    /// Cross-checks: province terrain exists, state provinces exist and belong to one state, history tags exist.
    pub fn check_references(&self, report: &mut Report) {
        for p in self.provinces.by_id.values() {
            if self.terrain.get(&p.terrain).is_none() {
                report.push("map/definition.csv", 0, "map.definition.terrain", IssueKind::BrokenReference("terrain"));
            }
        }
        let mut owner_of = std::collections::BTreeMap::new();
        for s in self.states.by_id.values() {
            for p in &s.provinces {
                if self.provinces.get(*p).is_none() {
                    report.push(&s.file, 0, "state.provinces", IssueKind::BrokenReference("province"));
                }
                if owner_of.insert(*p, s.id).is_some() {
                    report.push(&s.file, 0, "state.provinces", IssueKind::Redefinition("province in two states"));
                }
            }
            let h = &s.history;
            for tag in h.owner.iter().chain(&h.controller).chain(&h.cores).chain(&h.claims) {
                if self.countries.get(*tag).is_none() {
                    report.push(&s.file, 0, "state.history", IssueKind::BrokenReference("country tag"));
                }
            }
            for (p, _) in &h.victory_points {
                if !s.provinces.contains(p) {
                    report.push(
                        &s.file,
                        0,
                        "state.history.victory_points",
                        IssueKind::BrokenReference("province of this state"),
                    );
                }
            }
        }
    }
}
