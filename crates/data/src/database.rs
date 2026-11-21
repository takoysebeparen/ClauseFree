//! Loading everything and checking references between definitions.

use vfs::Vfs;

use crate::buildings::{self, Buildings};
use crate::countries::{self, Countries};
use crate::defines::{self, Defines};
use crate::equipment::{self, Equipment};
use crate::equipment::{LimitMatch, ModuleSlots, SlotSpec};
use crate::localisation::{self, Localisation};
use crate::modules::{self, ConvertFrom, Modules};
use crate::oob::{self, Oobs};
use crate::provinces::{self, Provinces};
use crate::report::{IssueKind, Report};
use crate::resources::{self, Resources};
use crate::states::{self, States};
use crate::technologies::{self, Technologies};
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
    pub modules: Modules,
    pub defines: Defines,
    pub oobs: Oobs,
    pub buildings: Buildings,
    pub technologies: Technologies,
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
            modules: modules::load(vfs, &mut report),
            defines: defines::load(vfs, &mut report),
            oobs: oob::load(vfs, &mut report),
            buildings: buildings::load(vfs, &mut report),
            technologies: technologies::load(vfs, &mut report),
        };
        db.check_references(&mut report);
        (db, report)
    }

    /// Cross-checks: province terrain exists, state provinces exist and belong to one state, history tags exist,
    /// equipment and sub-unit references resolve.
    pub fn check_references(&self, report: &mut Report) {
        self.check_unit_references(report);
        self.check_oob_references(report);
        self.check_building_references(report);
        self.check_module_references(report);
        self.check_technology_references(report);
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

    /// Buildings named by other buildings, by terrain and by state history exist; a state history puts a building at
    /// the state or province level that matches its kind (`province_max` set means provincial).
    fn check_building_references(&self, report: &mut Report) {
        let broken = |report: &mut Report, file: &str, path: &str, what: &'static str| {
            report.push(file, 0, path, IssueKind::BrokenReference(what));
        };
        for b in &self.buildings.list {
            for d in &b.prerequisites {
                if self.buildings.get(&d.building).is_none() {
                    broken(report, &b.file, "building.building_prerequisite.building_dependency.building", "building");
                }
            }
            for other in &b.level_cap.exclusive_with {
                if self.buildings.get(other).is_none() {
                    broken(report, &b.file, "building.level_cap.exclusive_with", "building");
                }
            }
            if let Some(sp) = b.text("spawn_point")
                && !self.buildings.spawn_points.contains_key(sp)
            {
                broken(report, &b.file, "building.spawn_point", "spawn point");
            }
            for (r, _) in &b.local_resources {
                if self.resources.get(r).is_none() {
                    broken(report, &b.file, "building.local_resources_<resource>", "resource");
                }
            }
        }
        for c in &self.terrain.categories {
            for (name, _) in &c.buildings_max_level {
                if self.buildings.get(name).is_none() {
                    broken(report, "common/terrain", "terrain.buildings_max_level", "building");
                }
            }
        }
        for s in self.states.by_id.values() {
            let start = &s.history.buildings;
            for sb in &start.state {
                match self.buildings.get(&sb.building) {
                    None => broken(report, &s.file, "state.history.buildings.<building>", "building"),
                    Some(b) if b.is_provincial() => {
                        broken(report, &s.file, "state.history.buildings.<building>", "state building")
                    }
                    Some(_) => {}
                }
            }
            for list in start.provinces.values() {
                for pb in list {
                    match self.buildings.get(&pb.building) {
                        None => broken(report, &s.file, "state.history.buildings.<province>.<building>", "building"),
                        Some(b) if !b.is_provincial() => broken(
                            report,
                            &s.file,
                            "state.history.buildings.<province>.<building>",
                            "provincial building",
                        ),
                        Some(_) => {}
                    }
                }
            }
        }
    }

    /// Modules and module categories named by modules and by modular equipment exist; every equipment's slots
    /// resolve; default modules go into slots the equipment has. A category exists if some module belongs to it.
    fn check_module_references(&self, report: &mut Report) {
        let broken = |report: &mut Report, file: &str, path: &str, what: &'static str| {
            report.push(file, 0, path, IssueKind::BrokenReference(what));
        };
        let categories = self.modules.categories();
        let category_ok = |c: &str| categories.contains(c);
        let module_ok = |m: &str| self.modules.get(m).is_some();
        for m in &self.modules.list {
            if let Some(p) = m.text("parent")
                && !module_ok(p)
            {
                broken(report, &m.file, "module.parent", "module");
            }
            for c in &m.conversions {
                match &c.from {
                    ConvertFrom::Module(n) if !module_ok(n) => {
                        broken(report, &m.file, "module.can_convert_from.module", "module")
                    }
                    ConvertFrom::Category(n) if !category_ok(n) => {
                        broken(report, &m.file, "module.can_convert_from.module_category", "module category")
                    }
                    _ => {}
                }
            }
            let named_categories = m
                .list("forbid_module_categories")
                .iter()
                .chain(m.allowed_module_categories.iter().flat_map(|(_, c)| c))
                .chain(m.forbid_exact_match_for_category.iter().map(|(c, _)| c));
            for c in named_categories {
                if !category_ok(c) {
                    broken(report, &m.file, "module.<module category>", "module category");
                }
            }
            for (r, _) in m.build_cost_resources.iter().chain(&m.dismantle_cost_resources) {
                if self.resources.get(r).is_none() {
                    broken(report, &m.file, "module.<cost resources>", "resource");
                }
            }
        }
        for e in self.equipment.by_name.values() {
            if let Some(ModuleSlots::List(slots)) = &e.module_slots {
                for s in slots {
                    if let SlotSpec::Defined { allowed_categories, .. } = &s.spec {
                        for c in allowed_categories {
                            if !category_ok(c) {
                                broken(
                                    report,
                                    &e.file,
                                    "equipment.module_slots.<slot>.allowed_module_categories",
                                    "module category",
                                );
                            }
                        }
                    }
                }
            }
            for l in &e.module_count_limits {
                for m in &l.matches {
                    match m {
                        LimitMatch::Module(n) if n != "any" && n != "empty" && !module_ok(n) => {
                            broken(report, &e.file, "equipment.module_count_limit.module", "module")
                        }
                        LimitMatch::Category(n) if n != "any" && !category_ok(n) => {
                            broken(report, &e.file, "equipment.module_count_limit.category", "module category")
                        }
                        _ => {}
                    }
                }
            }
            if e.module_slots.is_none() && e.default_modules.is_empty() {
                continue;
            }
            let Some(eff) = self.equipment.effective(&e.name) else {
                broken(report, &e.file, "equipment.module_slots", "inherited slot");
                continue;
            };
            let slots = match &eff.module_slots {
                Some(ModuleSlots::List(l)) => l.as_slice(),
                _ => &[],
            };
            for (slot, module) in &e.default_modules {
                if !slots.iter().any(|s| &s.name == slot) {
                    broken(report, &e.file, "equipment.default_modules.<slot>", "slot of this equipment");
                }
                // `inherit` was resolved by `effective` above.
                if module != "empty" && module != "inherit" && !module_ok(module) {
                    broken(report, &e.file, "equipment.default_modules.<slot>", "module");
                }
            }
        }
    }

    fn check_oob_references(&self, report: &mut Report) {
        for oob in self.oobs.by_name.values() {
            for t in &oob.templates {
                for slot in t.regiments.iter().chain(&t.support).chain(&t.regimental_support) {
                    if !self.units.sub_units.contains_key(&slot.sub_unit) {
                        report.push(
                            &oob.file,
                            0,
                            "oob.division_template.<sub_unit>",
                            IssueKind::BrokenReference("sub-unit"),
                        );
                    }
                }
            }
            // Templates are looked up among this file and the other OOB files of the same country (same tag prefix),
            // because some files only add divisions to templates defined in the country's main OOB.
            let tag = oob.name.split('_').next().unwrap_or("");
            let known = |name: &str| {
                self.oobs
                    .by_name
                    .values()
                    .filter(|o| o.name == oob.name || o.name.split('_').next() == Some(tag))
                    .any(|o| o.templates.iter().any(|t| t.name == name))
            };
            for d in &oob.divisions {
                if !d.template.is_empty() && !known(&d.template) {
                    // Not defined by any OOB of the country: some templates are created by script effects
                    // (decisions, focuses) before such an OOB is loaded, so this is resolved by the script VM.
                    report.push(&oob.file, 0, "oob.division.division_template (not in any OOB)", IssueKind::Deferred);
                }
                if let Some(p) = d.location
                    && self.provinces.get(p).is_none()
                {
                    report.push(&oob.file, 0, "oob.division.location", IssueKind::BrokenReference("province"));
                }
                for v in &d.force_equipment_variants {
                    if self.equipment.get(&v.equipment).is_none() {
                        report.push(
                            &oob.file,
                            0,
                            "oob.division.force_equipment_variants",
                            IssueKind::BrokenReference("equipment"),
                        );
                    }
                }
            }
        }
    }

    fn check_technology_references(&self, report: &mut Report) {
        let broken = |report: &mut Report, file: &str, path: &str, what: &'static str| {
            report.push(file, 0, path, IssueKind::BrokenReference(what));
        };
        let techs = &self.technologies;
        let is_unit_category = |c: &str| self.units.categories.iter().any(|k| k == c);
        let is_terrain = |k: &str| self.terrain.get(k).is_some() || COMBAT_SITUATIONS.contains(&k);
        for t in &techs.list {
            let f = t.file.as_str();
            for c in &t.categories {
                if !techs.has_category(c) {
                    broken(report, f, "technology.categories", "technology category");
                }
            }
            for p in &t.folders {
                if techs.folder(&p.folder).is_none() {
                    broken(report, f, "technology.folder.name", "technology folder");
                }
            }
            let tech_refs = t
                .paths
                .iter()
                .map(|p| ("technology.path.leads_to_tech", p.leads_to.as_str()))
                .chain(t.xor.iter().map(|x| ("technology.XOR", x.as_str())))
                .chain(t.dependencies.iter().map(|(d, _)| ("technology.dependencies", d.as_str())))
                .chain(t.sub_technologies.iter().map(|s| ("technology.sub_technologies", s.as_str())));
            for (path, name) in tech_refs {
                if techs.get(name).is_none() {
                    broken(report, f, path, "technology");
                }
            }
            for name in t.enabled_equipment() {
                if !self.equipment.exists(name) {
                    broken(report, f, "technology.enable_equipments", "equipment");
                }
            }
            for name in t.enable_equipment_modules.iter().flat_map(|u| &u.names) {
                if self.modules.get(name).is_none() {
                    broken(report, f, "technology.enable_equipment_modules", "module");
                }
            }
            for name in t.enable_subunits.iter().flat_map(|u| &u.names) {
                if !self.units.sub_units.contains_key(name) {
                    broken(report, f, "technology.enable_subunits", "sub-unit");
                }
            }
            for (b, _) in &t.enable_buildings {
                if self.buildings.get(b).is_none() {
                    broken(report, f, "technology.enable_building.building", "building");
                }
            }
            for u in &t.unit_stats {
                if !self.units.sub_units.contains_key(&u.target) && !is_unit_category(&u.target) {
                    broken(report, f, "technology.<unit>", "sub-unit or sub-unit category");
                }
                for k in u.terrain.keys() {
                    if !is_terrain(k) {
                        broken(report, f, "technology.<unit>.<terrain>", "terrain");
                    }
                }
                for m in &u.battalion_mult {
                    if !is_unit_category(&m.category) {
                        broken(report, f, "technology.<unit>.battalion_mult.category", "sub-unit category");
                    }
                }
            }
        }
    }
}
