//! Sub-units (battalions, support companies, ships, air wings): `common/units/*.txt`, block `sub_units`, and the
//! sub-unit categories of `common/unit_tags/*.txt`.
//!
//! Sources: the files themselves (key inventory of revisions `7c6c1c59` and `8873ec77`) and the community wiki page "Unit modding"
//! (<https://hoi4.paradoxwikis.com/Unit_modding>). Per the wiki, many numeric keys of a sub-unit are *modifiers*
//! applied on top of what its equipment provides (for example `soft_attack = -0.1` means 10% less soft attack than
//! the equipment gives), while others are absolute (`max_organisation`, `combat_width`, `manpower`). The data layer
//! stores the numbers exactly as written; which rule applies is the combat system's job and must be verified there.

use std::collections::BTreeMap;

use clausewitz::Block;
use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::NumberList;

/// Numeric keys of a sub-unit. Unit stats from the wiki's "Modifiers" list plus the other numeric properties seen in
/// the files.
pub const SUB_UNIT_NUMBERS: [&str; 48] = [
    // base, costs (wiki)
    "max_organisation",
    "reliability",
    "weight",
    "maximum_speed",
    "supply_consumption",
    "default_morale",
    "combat_width",
    "lend_lease_cost",
    "build_cost_ic",
    "manpower",
    "training_time",
    "max_strength",
    // offensive / defensive (wiki)
    "attack",
    "soft_attack",
    "hard_attack",
    "air_attack",
    "ap_attack",
    "breakthrough",
    "defense",
    "armor_value",
    "hardness",
    "entrenchment",
    // unique (wiki)
    "movement",
    "experience_loss_factor",
    "casualty_trickleback",
    "suppression_factor",
    "reliability_factor",
    "recon",
    "initiative",
    // seen in the files
    "suppression",
    "recovery",
    "supply_consumption_factor",
    "fuel_consumption_factor",
    "equipment_capture_factor",
    "own_equipment_fuel_consumption_mult",
    "hit_profile_mult",
    "critical_part_damage_chance_mult",
    "acclimatization_cold_climate_gain_factor",
    "acclimatization_hot_climate_gain_factor",
    "priority",
    "ai_priority",
    "deployment_cost",
    "division_3d_model_priority",
    "carrier_air_wing_size",
    "mega_carrier_air_wing_size",
    "submarine_carrier_air_wing_size",
    "land_air_wing_size",
    "default_carrier_composition_weight",
];

/// Yes/no switches of a sub-unit (wiki "Units" and "Boolean", plus the files).
pub const SUB_UNIT_FLAGS: [&str; 16] = [
    "active",
    "cavalry",
    "special_forces",
    "marines",
    "can_exfiltrate_from_coast",
    "mountaineers",
    "rangers",
    "can_be_parachuted",
    "affects_speed",
    "divisional",
    "regimental",
    "is_artillery_brigade",
    "allow_in_army_hq",
    "allow_in_non_army_hq",
    "can_license",
    "is_convertable",
];

/// Combat situations that take a modifier block like terrain categories do, but are not terrain categories in
/// `common/terrain`: crossing a river, landing from the sea, attacking a fort, and fighting in snow. Observed in the
/// files (revisions `7c6c1c59` and `8873ec77`); the list is engine-defined and may be incomplete.
pub const COMBAT_SITUATIONS: [&str; 4] = ["river", "amphibious", "fort", "snow"];

/// `battalion_mult = { category = X add = yes <stat> = N }`: modifies other battalions of a category.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BattalionMult {
    pub category: String,
    pub add: bool,
    pub values: NumberList,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubUnitDef {
    pub name: String,
    pub file: String,
    pub abbreviation: Option<String>,
    pub sprite: Option<String>,
    pub map_icon_category: Option<String>,
    /// Division designer group (`infantry`, `support`, `mobile`, `armor`, or a mod-defined one).
    pub group: Option<String>,
    /// Hardcoded internal types (`infantry`, `armor`, `fighter`, ...).
    pub types: Vec<String>,
    pub categories: Vec<String>,
    /// Equipment that makes the unit available (`essential`).
    pub essential: Vec<String>,
    /// Equipment needed per full-strength unit (`need = { infantry_equipment = 100 }`).
    pub need: NumberList,
    /// Naval/air form of `need`.
    pub need_equipment: NumberList,
    /// Equipment that sets the unit's speed (`transport`).
    pub transport: Option<String>,
    pub same_support_type: Option<String>,
    pub flags: Vec<(String, bool)>,
    /// Numbers from [`SUB_UNIT_NUMBERS`], as written.
    pub numbers: NumberList,
    /// Per-terrain (or per-situation, see [`COMBAT_SITUATIONS`]) modifiers, e.g. `forest = { attack = -0.2 }`.
    pub terrain: BTreeMap<String, NumberList>,
    pub battalion_mult: Vec<BattalionMult>,
    pub critical_parts: Vec<String>,
    pub required_dlc: Vec<String>,
    pub enable_ability: Vec<String>,
    pub allowed_battalion_groups: Vec<String>,
}

impl SubUnitDef {
    pub fn number(&self, key: &str) -> Option<common::Fixed> {
        self.numbers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Units {
    /// Sub-unit categories (`category_*`), in definition order.
    pub categories: Vec<String>,
    pub sub_units: BTreeMap<String, SubUnitDef>,
}

const UNITS_DIR: &str = "common/units";
const TAGS_DIR: &str = "common/unit_tags";
const S: &str = "sub_unit";

pub fn load(vfs: &Vfs, report: &mut Report) -> Units {
    let mut units = Units::default();
    for file in evaluation_order(vfs.list(TAGS_DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_categories(&src, report, &mut units);
    }
    for file in evaluation_order(vfs.list(UNITS_DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_sub_units(&src, report, &mut units);
    }
    units
}

pub fn parse_categories(src: &Source, report: &mut Report, units: &mut Units) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for e in top.take_all("sub_unit_categories") {
        for name in ctx.names(e, "sub_unit_categories") {
            if !units.categories.contains(&name) {
                units.categories.push(name);
            }
        }
    }
    top.finish(&mut ctx);
}

pub fn parse_sub_units(src: &Source, report: &mut Report, units: &mut Units) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for list in top.take_all("sub_units") {
        let Some(block) = ctx.block(list, "sub_units") else { continue };
        for entry in block.entries() {
            let Some(body) = ctx.block(entry, S) else { continue };
            let def = parse_sub_unit(entry.key.text, body, &mut ctx);
            if units.sub_units.contains_key(&def.name) {
                ctx.issue(entry.span, S, IssueKind::Redefinition("first"));
            } else {
                units.sub_units.insert(def.name.clone(), def);
            }
        }
    }
    top.finish(&mut ctx);
}

fn parse_sub_unit(name: &str, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> SubUnitDef {
    let mut f = Fields::new(block, S);
    let mut u = SubUnitDef { name: name.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    let text_fields: [(&str, &mut Option<String>); 6] = [
        ("abbreviation", &mut u.abbreviation),
        ("sprite", &mut u.sprite),
        ("map_icon_category", &mut u.map_icon_category),
        ("group", &mut u.group),
        ("transport", &mut u.transport),
        ("same_support_type", &mut u.same_support_type),
    ];
    for (key, slot) in text_fields {
        if let Some(e) = f.take(key) {
            *slot = ctx.text(e, &format!("{S}.{key}"));
        }
    }
    let lists: [(&str, &mut Vec<String>); 7] = [
        ("type", &mut u.types),
        ("categories", &mut u.categories),
        ("essential", &mut u.essential),
        ("critical_parts", &mut u.critical_parts),
        ("required_dlc", &mut u.required_dlc),
        ("enable_ability", &mut u.enable_ability),
        ("allowed_battalion_groups", &mut u.allowed_battalion_groups),
    ];
    for (key, slot) in lists {
        for e in f.take_all(key) {
            slot.extend(ctx.names(e, &format!("{S}.{key}")));
        }
    }
    for (key, slot) in [("need", &mut u.need), ("need_equipment", &mut u.need_equipment)] {
        if let Some(e) = f.take(key)
            && let Some(b) = ctx.block(e, &format!("{S}.{key}"))
        {
            *slot = ctx.numbers(b, &format!("{S}.{key}"));
        }
    }
    for key in SUB_UNIT_FLAGS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.bool(e, &format!("{S}.{key}"))
        {
            u.flags.push((key.to_string(), v));
        }
    }
    for key in SUB_UNIT_NUMBERS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.fixed(e, &format!("{S}.{key}"))
        {
            u.numbers.push((key.to_string(), v));
        }
    }
    for e in f.take_all("battalion_mult") {
        let Some(b) = ctx.block(e, &format!("{S}.battalion_mult")) else { continue };
        let mut bf = Fields::new(b, format!("{S}.battalion_mult"));
        let mut m = BattalionMult::default();
        if let Some(c) = bf.take("category") {
            m.category = ctx.text(c, &format!("{S}.battalion_mult.category")).unwrap_or_default();
        }
        if let Some(a) = bf.take("add") {
            m.add = ctx.bool(a, &format!("{S}.battalion_mult.add")).unwrap_or(false);
        }
        for v in bf.take_matching(|_| true) {
            if let Some(n) = ctx.fixed(v, &format!("{S}.battalion_mult.<stat>")) {
                m.values.push((v.key.text.to_string(), n));
            }
        }
        bf.finish(ctx);
        u.battalion_mult.push(m);
    }
    // Modifier blocks for leaders deployed with the unit, and module requirements, need the modifier and
    // equipment-module systems.
    f.defer_matching(|k| k == "deployed_leader_modifiers" || k == "need_equipment_modules");
    // Every remaining block of plain numbers is a terrain / situation modifier block; references are checked when the
    // database is assembled.
    // Plain values left over are duplicates or unknown keys; `finish` reports them.
    for e in f.take_entries(|e| e.value.as_block().is_some()) {
        if let Some(b) = e.value.as_block() {
            let list = ctx.numbers(b, &format!("{S}.<terrain>"));
            u.terrain.insert(e.key.text.to_string(), list);
        }
    }
    f.finish(ctx);
    u
}
