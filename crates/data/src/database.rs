//! Loading everything and checking references between definitions.

use std::collections::BTreeSet;

use vfs::Vfs;

use crate::adjacencies::{self, Adjacencies, AdjacencyKind};
use crate::buildings::{self, Buildings};
use crate::characters::{self, Characters};
use crate::combat_tactics::{self, CombatTactics};
use crate::continents::{self, Continents};
use crate::countries::{self, Countries};
use crate::country_history::{self, CountryHistories};
use crate::country_leader::{self, CountryLeaderTraits};
use crate::defines::{self, Defines};
use crate::doctrines::{self, Doctrines};
use crate::equipment::{self, Equipment};
use crate::equipment::{LimitMatch, ModuleSlots, SlotSpec};
use crate::equipment_groups::{self, EquipmentGroups};
use crate::ideas::{self, Ideas};
use crate::ideologies::{self, Ideologies};
use crate::localisation::{self, Localisation};
use crate::modules::{self, ConvertFrom, Modules};
use crate::oob::{self, Oobs};
use crate::provinces::{self, ProvinceKind, Provinces};
use crate::report::{IssueKind, Report};
use crate::resources::{self, Resources};
use crate::states::{self, States};
use crate::strategic_regions::{self, StrategicRegions};
use crate::supply::{self, Supply};
use crate::technologies::{self, Technologies};
use crate::terrain::{self, Terrain};
use crate::unit_leader::{self, UnitLeaders};
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
    pub equipment_groups: EquipmentGroups,
    pub ideas: Ideas,
    pub characters: Characters,
    pub country_history: CountryHistories,
    pub continents: Continents,
    pub adjacencies: Adjacencies,
    pub strategic_regions: StrategicRegions,
    pub supply: Supply,
    pub ideologies: Ideologies,
    pub combat_tactics: CombatTactics,
    pub unit_leaders: UnitLeaders,
    pub country_leader_traits: CountryLeaderTraits,
    pub doctrines: Doctrines,
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
            equipment_groups: equipment_groups::load(vfs, &mut report),
            ideas: ideas::load(vfs, &mut report),
            characters: characters::load(vfs, &mut report),
            country_history: country_history::load(vfs, &mut report),
            continents: continents::load(vfs, &mut report),
            adjacencies: adjacencies::load(vfs, &mut report),
            strategic_regions: strategic_regions::load(vfs, &mut report),
            supply: supply::load(vfs, &mut report),
            ideologies: ideologies::load(vfs, &mut report),
            combat_tactics: combat_tactics::load(vfs, &mut report),
            unit_leaders: unit_leader::load(vfs, &mut report),
            country_leader_traits: country_leader::load(vfs, &mut report),
            doctrines: doctrines::load(vfs, &mut report),
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
        self.check_idea_references(report);
        self.check_character_references(report);
        self.check_country_history_references(report);
        self.check_map_references(report);
        self.check_tactic_references(report);
        self.check_unit_leader_references(report);
        self.check_ideology_references(report);
        self.check_country_leader_trait_references(report);
        self.check_doctrine_references(report);
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

impl Database {
    /// Names an `equipment_bonus` or an equipment group may use: an equipment (or duplicate-archetype copy), an
    /// equipment `type`, an equipment `alias` (`alias = destroyer` in `ship_hull_light.txt`) or an equipment group.
    fn equipment_targets(&self) -> BTreeSet<&str> {
        let mut out: BTreeSet<&str> = BTreeSet::new();
        for e in self.equipment.by_name.values() {
            out.insert(e.name.as_str());
            out.extend(e.types.iter().map(String::as_str));
            out.extend(e.text("alias"));
        }
        out.extend(self.equipment_groups.list.iter().map(|g| g.name.as_str()));
        out
    }

    /// Idea groups are idea categories or slots; research bonuses name technology categories; equipment bonuses and
    /// equipment groups name equipment (see [`Database::equipment_targets`]).
    fn check_idea_references(&self, report: &mut Report) {
        let ideas = &self.ideas;
        let is_group =
            |g: &str| ideas.category(g).is_some() || ideas.category_of_slot(g).is_some() || ideas.has_character_slot(g);
        let targets = self.equipment_targets();
        let is_equipment_target = |t: &str| targets.contains(t) || self.equipment.exists(t);
        for g in &self.equipment_groups.list {
            for m in &g.members {
                if !is_equipment_target(m) {
                    report.push(
                        &g.file,
                        0,
                        "equipment_group.equipment_type",
                        IssueKind::BrokenReference("equipment, equipment type or group"),
                    );
                }
            }
        }
        for d in &ideas.list {
            let f = d.file.as_str();
            if !is_group(&d.group) {
                report.push(f, 0, "ideas.<group>", IssueKind::BrokenReference("idea category or slot"));
            }
            for (c, _) in &d.research_bonus {
                if !self.technologies.has_category(c) {
                    report.push(f, 0, "idea.research_bonus", IssueKind::BrokenReference("technology category"));
                }
            }
            for b in &d.equipment_bonus {
                if !is_equipment_target(&b.target) {
                    report.push(
                        f,
                        0,
                        "idea.equipment_bonus.<equipment>",
                        IssueKind::BrokenReference("equipment, equipment type or group"),
                    );
                }
            }
        }
    }

    /// Advisor slots are character slots of an idea category; research bonuses name technology categories.
    fn check_character_references(&self, report: &mut Report) {
        for c in &self.characters.list {
            for a in c.bodies().flat_map(|b| &b.advisors) {
                if let Some(slot) = &a.slot
                    && !self.ideas.has_character_slot(slot)
                {
                    report.push(&c.file, 0, "character.advisor.slot", IssueKind::BrokenReference("character slot"));
                }
                for (cat, _) in &a.research_bonus {
                    if !self.technologies.has_category(cat) {
                        report.push(
                            &c.file,
                            0,
                            "character.advisor.research_bonus",
                            IssueKind::BrokenReference("technology category"),
                        );
                    }
                }
            }
        }
    }

    /// The tag, capital, OOB, technologies, ideas and characters named by a country history exist. `add_ideas` may
    /// also name the `idea_token` of an advisor role (`add_ideas = SOV_genrikh_yagoda` in the Soviet history).
    fn check_country_history_references(&self, report: &mut Report) {
        let advisor_ideas: BTreeSet<&str> = self
            .characters
            .list
            .iter()
            .flat_map(|c| c.bodies())
            .flat_map(|b| &b.advisors)
            .filter_map(|a| a.idea_token.as_deref())
            .collect();
        for h in self.country_history.by_tag.values() {
            let broken = |report: &mut Report, path: &str, what: &'static str| {
                report.push(&h.file, 0, path, IssueKind::BrokenReference(what));
            };
            if let Some(tag) = h.tag
                && self.countries.get(tag).is_none()
            {
                broken(report, "country_history", "country tag");
            }
            if let Some(id) = h.capital
                && self.states.get(id).is_none()
            {
                broken(report, "country_history.capital", "state");
            }
            if let Some(oob) = &h.oob
                && !self.oobs.by_name.contains_key(oob)
            {
                broken(report, "country_history.oob", "OOB file");
            }
            for (t, _) in &h.technologies {
                if self.technologies.get(t).is_none() {
                    broken(report, "country_history.set_technology.<technology>", "technology");
                }
            }
            for i in &h.ideas {
                if self.ideas.get(i).is_none() && !advisor_ideas.contains(i.as_str()) {
                    broken(report, "country_history.add_ideas", "idea");
                }
            }
            for c in &h.characters {
                if self.characters.get(c).is_none() {
                    broken(report, "country_history.recruit_character", "character");
                }
            }
        }
    }

    /// Map topology: continent indices, adjacency provinces and rules, strategic region provinces and naval terrain,
    /// railway and supply node provinces. Every province except `0` belongs to exactly one strategic region.
    fn check_map_references(&self, report: &mut Report) {
        for p in self.provinces.by_id.values() {
            if p.continent != 0 && self.continents.by_index(p.continent).is_none() {
                report.push(
                    "map/definition.csv",
                    0,
                    "map.definition.continent",
                    IssueKind::BrokenReference("continent"),
                );
            }
        }
        let exists = |id| self.provinces.get(id).is_some();
        for a in &self.adjacencies.list {
            let broken = |report: &mut Report, path: &str, what: &'static str| {
                report.push("map/adjacencies.csv", a.line, path, IssueKind::BrokenReference(what));
            };
            if !exists(a.from) || !exists(a.to) {
                broken(report, "map.adjacencies.from_to", "province");
            }
            match a.through {
                Some(t) if !exists(t) => broken(report, "map.adjacencies.through", "province"),
                None if a.kind == AdjacencyKind::Sea => report.push(
                    "map/adjacencies.csv",
                    a.line,
                    "map.adjacencies.through",
                    IssueKind::Missing("sea province"),
                ),
                _ => {}
            }
            if let Some(rule) = &a.rule
                && self.adjacencies.rule(rule).is_none()
            {
                broken(report, "map.adjacencies.adjacency_rule_name", "adjacency rule");
            }
        }
        for r in &self.adjacencies.rules {
            if r.required_provinces.iter().chain(r.icon.iter()).any(|p| !exists(*p)) {
                report.push(&r.file, 0, "adjacency_rule.<province>", IssueKind::BrokenReference("province"));
            }
        }
        let mut region_of = std::collections::BTreeMap::new();
        for r in self.strategic_regions.by_id.values() {
            for p in &r.provinces {
                if !exists(*p) {
                    report.push(&r.file, 0, "strategic_region.provinces", IssueKind::BrokenReference("province"));
                }
                if region_of.insert(*p, r.id).is_some() {
                    report.push(
                        &r.file,
                        0,
                        "strategic_region.provinces",
                        IssueKind::Redefinition("province in two regions"),
                    );
                }
            }
            if let Some(t) = &r.naval_terrain
                && self.terrain.get(t).is_none()
            {
                report.push(&r.file, 0, "strategic_region.naval_terrain", IssueKind::BrokenReference("terrain"));
            }
        }
        if !self.strategic_regions.is_empty() {
            for id in self.provinces.by_id.keys().filter(|id| id.0 != 0) {
                if !region_of.contains_key(id) {
                    report.push("map/definition.csv", 0, "map.definition", IssueKind::Missing("strategic region"));
                }
            }
        }
        let is_land = |id| self.provinces.get(id).is_some_and(|p| p.kind == ProvinceKind::Land);
        for r in &self.supply.railways {
            if r.provinces.iter().any(|p| !is_land(*p)) {
                report.push(supply::RAILWAYS, r.line, "map.railways", IssueKind::BrokenReference("land province"));
            }
        }
        for n in &self.supply.nodes {
            if !is_land(n.province) {
                report.push(
                    supply::SUPPLY_NODES,
                    n.line,
                    "map.supply_nodes",
                    IssueKind::BrokenReference("land province"),
                );
            }
        }
    }

    /// Counter tactics exist, phases are declared in `phases` (`no` is the standard phase), and `only_show_for` names
    /// a country.
    fn check_tactic_references(&self, report: &mut Report) {
        let t = &self.combat_tactics;
        let file = combat_tactics::FILE;
        for tactic in &t.list {
            if let Some(c) = &tactic.countered_by
                && t.get(c).is_none()
            {
                report.push(file, 0, "combat_tactic.countered_by", IssueKind::BrokenReference("combat tactic"));
            }
            for (path, phase) in
                [("combat_tactic.phase", &tactic.phase), ("combat_tactic.display_phase", &tactic.display_phase)]
            {
                if let Some(p) = phase
                    && p != "no"
                    && !t.phases.contains(p)
                {
                    report.push(file, 0, path, IssueKind::BrokenReference("combat phase"));
                }
            }
            if let Some(tag) = tactic.only_show_for
                && self.countries.get(tag).is_none()
            {
                report.push(file, 0, "combat_tactic.only_show_for", IssueKind::BrokenReference("country tag"));
            }
        }
    }

    /// Traits named by traits (parents, exclusions, experience factors) and by commander roles exist; trait sub-unit
    /// modifiers and unit types name sub-units; ship equipment exists; advisor slots are character slots.
    fn check_unit_leader_references(&self, report: &mut Report) {
        let leaders = &self.unit_leaders;
        let has_trait = |name: &str| leaders.get_trait(name).is_some();
        for t in &leaders.traits {
            let broken = |report: &mut Report, path: &str, what: &'static str| {
                report.push(&t.file, 0, path, IssueKind::BrokenReference(what));
            };
            for name in t.parents.iter().flat_map(|p| &p.traits) {
                if !has_trait(name) {
                    broken(report, "leader_trait.parent", "leader trait");
                }
            }
            for name in &t.mutually_exclusive {
                if !has_trait(name) {
                    broken(report, "leader_trait.mutually_exclusive", "leader trait");
                }
            }
            for (name, _) in &t.trait_xp_factor {
                if !has_trait(name) {
                    broken(report, "leader_trait.trait_xp_factor", "leader trait");
                }
            }
            for name in t.sub_unit_modifiers.iter().map(|(k, _)| k).chain(&t.unit_types) {
                if !self.units.sub_units.contains_key(name) {
                    broken(report, "leader_trait.<sub_unit>", "sub-unit");
                }
            }
            for name in &t.allowed_ship_equipments {
                if self.equipment.get(name).is_none() {
                    broken(report, "leader_trait.allowed_ship_equipments", "equipment");
                }
            }
            for (terrain, _) in t.modifier_blocks.iter().flat_map(|(_, m)| &m.terrain) {
                if self.terrain.get(terrain).is_none() && !COMBAT_SITUATIONS.contains(&terrain.as_str()) {
                    broken(report, "leader_trait.<modifier>.<terrain>", "terrain");
                }
            }
            if let Some(slot) = &t.advisor_slot
                && !self.ideas.has_character_slot(slot)
            {
                broken(report, "leader_trait.slot", "character slot");
            }
        }
        for c in &self.characters.list {
            for name in c.bodies().flat_map(|b| &b.commanders).flat_map(|r| &r.traits) {
                if !has_trait(name) {
                    report.push(&c.file, 0, "character.<commander>.traits", IssueKind::BrokenReference("leader trait"));
                }
            }
        }
    }

    /// Country leaders name a sub-ideology; country histories name ideologies for the ruling party and popularities.
    fn check_ideology_references(&self, report: &mut Report) {
        let ideologies = &self.ideologies;
        for c in &self.characters.list {
            for i in c.bodies().flat_map(|b| &b.country_leader).filter_map(|r| r.ideology.as_deref()) {
                if ideologies.of_type(i).is_none() {
                    report.push(
                        &c.file,
                        0,
                        "character.country_leader.ideology",
                        IssueKind::BrokenReference("ideology type"),
                    );
                }
            }
        }
        for h in self.country_history.by_tag.values() {
            if let Some(p) = h.politics.as_ref().and_then(|p| p.ruling_party.as_deref())
                && ideologies.get(p).is_none()
            {
                report.push(
                    &h.file,
                    0,
                    "country_history.set_politics.ruling_party",
                    IssueKind::BrokenReference("ideology"),
                );
            }
            for (i, _) in &h.popularities {
                if ideologies.get(i).is_none() {
                    report.push(&h.file, 0, "country_history.set_popularities", IssueKind::BrokenReference("ideology"));
                }
            }
        }
    }

    /// Traits named by country leader and advisor roles, by ideas and by unit leader traits (as advisor traits) are
    /// country leader traits; trait equipment bonuses name equipment; AI strategies name countries.
    fn check_country_leader_trait_references(&self, report: &mut Report) {
        let traits = &self.country_leader_traits;
        for c in &self.characters.list {
            for b in c.bodies() {
                let names = b
                    .country_leader
                    .iter()
                    .flat_map(|r| r.traits.iter().map(|t| ("character.country_leader.traits", t)))
                    .chain(b.advisors.iter().flat_map(|r| r.traits.iter().map(|t| ("character.advisor.traits", t))));
                for (path, name) in names {
                    if traits.get(name).is_none() {
                        report.push(&c.file, 0, path, IssueKind::BrokenReference("country leader trait"));
                    }
                }
            }
        }
        for d in &self.ideas.list {
            for name in &d.traits {
                if traits.get(name).is_none() {
                    report.push(&d.file, 0, "idea.traits", IssueKind::BrokenReference("country leader trait"));
                }
            }
        }
        for t in &self.unit_leaders.traits {
            for (_, name) in &t.advisor_traits {
                if traits.get(name).is_none() {
                    report.push(
                        &t.file,
                        0,
                        "leader_trait.<advisor_trait>",
                        IssueKind::BrokenReference("country leader trait"),
                    );
                }
            }
        }
        let targets = self.equipment_targets();
        for t in &traits.list {
            for b in &t.equipment_bonus {
                if !targets.contains(b.target.as_str()) && !self.equipment.exists(&b.target) {
                    report.push(
                        &t.file,
                        0,
                        "country_leader_trait.equipment_bonus.<equipment>",
                        IssueKind::BrokenReference("equipment, equipment type or group"),
                    );
                }
            }
            for id in t.ai_strategies.iter().filter_map(|a| a.id.as_deref()) {
                if crate::types::Tag::parse(id).and_then(|tag| self.countries.get(tag)).is_none() {
                    report.push(
                        &t.file,
                        0,
                        "country_leader_trait.ai_strategy.id",
                        IssueKind::BrokenReference("country tag"),
                    );
                }
            }
        }
    }

    /// Grand doctrines name a folder and tracks; subdoctrines name tracks and other subdoctrines; activation effects
    /// name combat tactics, sub-units or categories, terrain and equipment; mastery rules name sub-units, categories
    /// and equipment types.
    fn check_doctrine_references(&self, report: &mut Report) {
        let d = &self.doctrines;
        let is_unit_category = |c: &str| self.units.categories.iter().any(|k| k == c);
        let is_unit = |c: &str| self.units.sub_units.contains_key(c) || is_unit_category(c);
        let is_terrain = |k: &str| self.terrain.get(k).is_some() || COMBAT_SITUATIONS.contains(&k);
        let targets = self.equipment_targets();
        let equipment_types: BTreeSet<&str> =
            self.equipment.by_name.values().flat_map(|e| e.types.iter().map(String::as_str)).collect();
        let check_mastery = |report: &mut Report, file: &str, path: &str, m: &doctrines::MasteryRule| {
            for name in &m.sub_units {
                if !self.units.sub_units.contains_key(name) {
                    report.push(file, 0, format!("{path}.sub_units"), IssueKind::BrokenReference("sub-unit"));
                }
            }
            for name in &m.categories {
                if !is_unit_category(name) {
                    report.push(file, 0, format!("{path}.categories"), IssueKind::BrokenReference("sub-unit category"));
                }
            }
            for name in &m.equipment {
                if !equipment_types.contains(name.as_str()) {
                    report.push(file, 0, format!("{path}.equipment"), IssueKind::BrokenReference("equipment type"));
                }
            }
        };
        for g in &d.grand_doctrines {
            match &g.folder {
                Some(folder) if d.folder(folder).is_none() => {
                    report.push(&g.file, 0, "grand_doctrine.folder", IssueKind::BrokenReference("doctrine folder"));
                }
                Some(_) => {}
                None => report.push(&g.file, 0, "grand_doctrine", IssueKind::Missing("folder")),
            }
            for t in &g.tracks {
                if d.track(t).is_none() {
                    report.push(&g.file, 0, "grand_doctrine.tracks", IssueKind::BrokenReference("doctrine track"));
                }
            }
        }
        for t in &d.tracks {
            if let Some(m) = &t.mastery {
                check_mastery(report, &t.file, "doctrine_track.mastery", m);
            }
        }
        for s in &d.subdoctrines {
            for t in &s.tracks {
                if d.track(t).is_none() {
                    report.push(&s.file, 0, "subdoctrine.track", IssueKind::BrokenReference("doctrine track"));
                }
            }
            for x in &s.xor {
                if d.subdoctrine(x).is_none() {
                    report.push(&s.file, 0, "subdoctrine.xor", IssueKind::BrokenReference("subdoctrine"));
                }
            }
            if let Some(m) = &s.mastery {
                check_mastery(report, &s.file, "subdoctrine.mastery", m);
            }
        }
        for (file, e) in d.all_effects() {
            for t in &e.tactics {
                if self.combat_tactics.get(t).is_none() {
                    report.push(file, 0, "doctrine.enable_tactic", IssueKind::BrokenReference("combat tactic"));
                }
            }
            for u in &e.unit_stats {
                if !is_unit(&u.target) {
                    report.push(
                        file,
                        0,
                        "doctrine.<unit>",
                        IssueKind::BrokenReference("sub-unit or sub-unit category"),
                    );
                }
                for k in u.terrain.keys() {
                    if !is_terrain(k) {
                        report.push(file, 0, "doctrine.<unit>.<terrain>", IssueKind::BrokenReference("terrain"));
                    }
                }
            }
            for b in &e.equipment_bonus {
                if !targets.contains(b.target.as_str()) && !self.equipment.exists(&b.target) {
                    report.push(
                        file,
                        0,
                        "doctrine.equipment_bonus.<equipment>",
                        IssueKind::BrokenReference("equipment, equipment type or group"),
                    );
                }
            }
        }
    }
}
