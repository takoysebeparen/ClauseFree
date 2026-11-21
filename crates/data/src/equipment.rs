//! Equipment: `common/units/equipment/*.txt`, blocks `equipments` and `duplicate_archetypes`.
//!
//! Source: the syntax reference shipped with the game, `common/units/equipment/_documentation.md`, plus a key
//! inventory of the files (revisions `7c6c1c59` and `8873ec77`). Stats are stored as written. Inheritance (variant -> archetype,
//! `parent`, upgrades) and modular equipment (`module_slots`, `default_modules`, ...) are resolved by later layers;
//! the modular keys are reported as deferred until the module loader exists.

use std::collections::BTreeMap;

use clausewitz::Block;
use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::NumberList;

/// Numeric keys: unit stats (`<unit stat name> = <value>` in the documentation) and numeric properties seen in the
/// files.
pub const EQUIPMENT_NUMBERS: [&str; 58] = [
    // production and logistics
    "year",
    "priority",
    "build_cost_ic",
    "manpower",
    "lend_lease_cost",
    "reliability",
    "visual_level",
    "weight",
    "max_military_factories",
    "max_dockyard_factories",
    "fuel_consumption",
    "interface_overview_category_index",
    "air_map_icon_frame",
    "visual_tech_level_addition",
    "carrier_size",
    "default_carrier_composition_weight",
    // land
    "maximum_speed",
    "max_strength",
    "max_organisation",
    "soft_attack",
    "hard_attack",
    "ap_attack",
    "air_attack",
    "defense",
    "breakthrough",
    "armor_value",
    "hardness",
    // air
    "air_agility",
    "air_range",
    "air_defence",
    "air_bombing",
    "air_superiority",
    "air_ground_attack",
    "naval_strike_attack",
    "naval_strike_targetting",
    "additional_collateral_damage",
    // naval
    "naval_speed",
    "naval_range",
    "naval_dominance_factor",
    "naval_weather_penalty_factor",
    "surface_detection",
    "sub_detection",
    "surface_visibility",
    "sub_visibility",
    "torpedo_attack",
    "sub_attack",
    "anti_air_attack",
    "lg_attack",
    "lg_armor_piercing",
    "hg_attack",
    "hg_armor_piercing",
    "fire_range",
    "evasion",
    "shore_bombardment",
    // railway guns
    "railway_gun_attack",
    "railway_gun_annex_ratio",
    "railway_gun_attack_range_index_in_define",
    "railway_gun_hours_between_redistribution",
];

/// Yes/no switches.
pub const EQUIPMENT_FLAGS: [&str; 11] = [
    "is_archetype",
    "is_buildable",
    "is_frame",
    "active",
    "can_license",
    "carrier_capable",
    "is_convertable",
    "one_use_only",
    "offensive_weapons",
    "supply_truck",
    "only_duplicate_archetype",
];

/// Single-name keys.
pub const EQUIPMENT_TEXTS: [&str; 15] = [
    "archetype",
    "parent",
    "abbreviation",
    "picture",
    "sprite",
    "model",
    "group_by",
    "derived_variant_name",
    "family",
    "interface_category",
    "ai_type",
    "alias",
    "substitute",
    "type_override",
    "forbid_mission_type",
];

/// Keys that hold triggers (script) or modular-equipment data; loaded by later layers.
const DEFERRED: [&str; 11] = [
    "can_be_produced",
    "can_be_lend_leased",
    "module_slots",
    "module_count_limit",
    "default_modules",
    "module_add_stats_multiplier",
    "modifier_stat",
    "scaled_by_reliability",
    "for_each",
    "upgrade_window_override",
    "variant_name",
];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EquipmentDef {
    pub name: String,
    pub file: String,
    /// Defined under `duplicate_archetypes` rather than `equipments`.
    pub duplicate: bool,
    /// `type`: one or more of the documented equipment types.
    pub types: Vec<String>,
    pub upgrades: Vec<String>,
    pub can_convert_from: Vec<String>,
    pub allowed_types: Vec<String>,
    pub allow_mission_type: Vec<String>,
    /// Resources per unit produced.
    pub resources: NumberList,
    pub flags: Vec<(String, bool)>,
    pub texts: Vec<(String, String)>,
    pub numbers: NumberList,
}

impl EquipmentDef {
    pub fn flag(&self, key: &str) -> Option<bool> {
        self.flags.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.texts.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn number(&self, key: &str) -> Option<common::Fixed> {
        self.numbers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    /// Archetypes are marked `is_archetype = yes`; every entry of `duplicate_archetypes` is a copy of an archetype
    /// and therefore an archetype too (see the header of `x_plane_airframes.txt`: "the equipment attributes alias,
    /// type, variant_name, and derived_variant_name are not duplicated").
    pub fn is_archetype(&self) -> bool {
        self.duplicate || self.flag("is_archetype").unwrap_or(false)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Equipment {
    pub by_name: BTreeMap<String, EquipmentDef>,
}

impl Equipment {
    pub fn get(&self, name: &str) -> Option<&EquipmentDef> {
        self.by_name.get(name)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// The equipment with everything it inherits through its `archetype` chain filled in.
    ///
    /// Rules (community wiki "Equipment modding": "Archetype equipment is used to assign more general attributes
    /// that regular equipment then inherits via the archetype attribute"; `parent` is only "the previous version"
    /// and is not inherited from):
    /// - a value written on the equipment wins over the inherited one;
    /// - `is_archetype` and `is_buildable` describe the entry itself and are never inherited (the base game's
    ///   archetypes are all `is_buildable = no`, while their variants are buildable) — an assumption;
    /// - a duplicate archetype does not take `alias`, `type`, `variant_name` and `derived_variant_name` from the
    ///   archetype it copies (header of `x_plane_airframes.txt`);
    /// - list-valued attributes (`type`, `upgrades`, `resources`, ...) are replaced as a whole, not merged — an
    ///   assumption.
    ///
    /// `None` if the name is unknown or the chain is broken or circular.
    pub fn effective(&self, name: &str) -> Option<EquipmentDef> {
        self.resolve(name, 0)
    }

    fn resolve(&self, name: &str, depth: usize) -> Option<EquipmentDef> {
        // The base game's chains are at most three long; anything deeper is a loop.
        if depth > 16 {
            return None;
        }
        let own = self.get(name)?;
        let Some(base_name) = own.text("archetype") else { return Some(own.clone()) };
        let mut base = self.resolve(base_name, depth + 1)?;
        base.flags.retain(|(k, _)| k != "is_archetype" && k != "is_buildable");
        if own.duplicate {
            base.texts.retain(|(k, _)| k != "alias" && k != "derived_variant_name");
            base.types.clear();
        }
        let mut out = own.clone();
        merge_pairs(&mut out.numbers, &base.numbers);
        merge_pairs(&mut out.flags, &base.flags);
        merge_pairs(&mut out.texts, &base.texts);
        for (mine, inherited) in [
            (&mut out.types, &base.types),
            (&mut out.upgrades, &base.upgrades),
            (&mut out.can_convert_from, &base.can_convert_from),
            (&mut out.allowed_types, &base.allowed_types),
            (&mut out.allow_mission_type, &base.allow_mission_type),
        ] {
            if mine.is_empty() {
                mine.clone_from(inherited);
            }
        }
        if out.resources.is_empty() {
            out.resources.clone_from(&base.resources);
        }
        Some(out)
    }
}

/// Adds the inherited `(key, value)` pairs whose key the equipment does not set itself.
fn merge_pairs<V: Clone>(own: &mut Vec<(String, V)>, inherited: &[(String, V)]) {
    for (k, v) in inherited {
        if !own.iter().any(|(ok, _)| ok == k) {
            own.push((k.clone(), v.clone()));
        }
    }
}

const DIR: &str = "common/units/equipment";
const S: &str = "equipment";

pub fn load(vfs: &Vfs, report: &mut Report) -> Equipment {
    let mut out = Equipment::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

pub fn parse_file(src: &Source, report: &mut Report, out: &mut Equipment) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for (key, duplicate) in [("equipments", false), ("duplicate_archetypes", true)] {
        for list in top.take_all(key) {
            let Some(block) = ctx.block(list, key) else { continue };
            for entry in block.entries() {
                let Some(body) = ctx.block(entry, S) else { continue };
                let def = parse_equipment(entry.key.text, duplicate, body, &mut ctx);
                if out.by_name.contains_key(&def.name) {
                    ctx.issue(entry.span, S, IssueKind::Redefinition("first"));
                } else {
                    out.by_name.insert(def.name.clone(), def);
                }
            }
        }
    }
    // Filters of the production / design screens.
    top.defer_matching(|k| k == "search_filters");
    top.finish(&mut ctx);
}

fn parse_equipment(name: &str, duplicate: bool, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> EquipmentDef {
    let mut f = Fields::new(block, S);
    let mut e = EquipmentDef { name: name.to_string(), file: ctx.src.path.clone(), duplicate, ..Default::default() };
    let lists: [(&str, &mut Vec<String>); 5] = [
        ("type", &mut e.types),
        ("upgrades", &mut e.upgrades),
        ("can_convert_from", &mut e.can_convert_from),
        ("allowed_types", &mut e.allowed_types),
        ("allow_mission_type", &mut e.allow_mission_type),
    ];
    for (key, slot) in lists {
        for entry in f.take_all(key) {
            slot.extend(ctx.names(entry, &format!("{S}.{key}")));
        }
    }
    if let Some(r) = f.take("resources")
        && let Some(b) = ctx.block(r, "equipment.resources")
    {
        e.resources = ctx.numbers(b, "equipment.resources");
    }
    for key in EQUIPMENT_FLAGS {
        if let Some(entry) = f.take(key)
            && let Some(v) = ctx.bool(entry, &format!("{S}.{key}"))
        {
            e.flags.push((key.to_string(), v));
        }
    }
    for key in EQUIPMENT_TEXTS {
        for entry in f.take_all(key) {
            if let Some(v) = ctx.text(entry, &format!("{S}.{key}")) {
                e.texts.push((key.to_string(), v));
            }
        }
    }
    for key in EQUIPMENT_NUMBERS {
        if let Some(entry) = f.take(key)
            && let Some(v) = ctx.fixed(entry, &format!("{S}.{key}"))
        {
            e.numbers.push((key.to_string(), v));
        }
    }
    f.defer_matching(|k| DEFERRED.contains(&k));
    f.finish(ctx);
    e
}
