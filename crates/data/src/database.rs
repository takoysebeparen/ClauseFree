//! Loading everything and checking references between definitions.

use vfs::Vfs;

use crate::countries::{self, Countries};
use crate::defines::{self, Defines};
use crate::equipment::{self, Equipment};
use crate::localisation::{self, Localisation};
use crate::provinces::{self, Provinces};
use crate::report::{IssueKind, Report};
use crate::resources::{self, Resources};
use crate::states::{self, States};
use crate::terrain::{self, Terrain};
use crate::units::{self, COMBAT_SITUATIONS, Units};

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
    pub resources: Resources,
    pub units: Units,
    pub equipment: Equipment,
    pub defines: Defines,
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
            resources: resources::load(vfs, &mut report),
            units: units::load(vfs, &mut report),
            equipment: equipment::load(vfs, &mut report),
            defines: defines::load(vfs, &mut report),
        };
        db.check_references(&mut report);
        (db, report)
    }

    /// Cross-checks: province terrain exists, state provinces exist and belong to one state, history tags exist,
    /// equipment and sub-unit references resolve.
    pub fn check_references(&self, report: &mut Report) {
        self.check_unit_references(report);
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

    fn check_unit_references(&self, report: &mut Report) {
        let broken = |report: &mut Report, file: &str, path: &str, what: &'static str| {
            report.push(file, 0, path, IssueKind::BrokenReference(what));
        };
        for e in self.equipment.by_name.values() {
            if let Some(a) = e.text("archetype") {
                match self.equipment.get(a) {
                    Some(arch) if arch.is_archetype() => {}
                    Some(_) => broken(report, &e.file, "equipment.archetype", "archetype (not marked is_archetype)"),
                    None => broken(report, &e.file, "equipment.archetype", "equipment"),
                }
            }
            if let Some(p) = e.text("parent")
                && self.equipment.get(p).is_none()
            {
                broken(report, &e.file, "equipment.parent", "equipment");
            }
            for (r, _) in &e.resources {
                if self.resources.get(r).is_none() {
                    broken(report, &e.file, "equipment.resources", "resource");
                }
            }
        }
        for u in self.units.sub_units.values() {
            for c in &u.categories {
                if !self.units.categories.contains(c) {
                    broken(report, &u.file, "sub_unit.categories", "sub-unit category");
                }
            }
            let equipment_refs =
                u.need.iter().chain(&u.need_equipment).map(|(k, _)| k).chain(&u.essential).chain(u.transport.iter());
            for name in equipment_refs {
                if self.equipment.get(name).is_none() {
                    broken(report, &u.file, "sub_unit.<equipment>", "equipment");
                }
            }
            for key in u.terrain.keys() {
                if self.terrain.get(key).is_none() && !COMBAT_SITUATIONS.contains(&key.as_str()) {
                    broken(report, &u.file, "sub_unit.<terrain>", "terrain");
                }
            }
            for m in &u.battalion_mult {
                if !self.units.categories.contains(&m.category) {
                    broken(report, &u.file, "sub_unit.battalion_mult.category", "sub-unit category");
                }
            }
        }
    }
}
