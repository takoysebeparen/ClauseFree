//! Buildings: `common/buildings/*.txt`, blocks `buildings` and `spawn_points`.
//!
//! Source: the files themselves (revision `7c6c1c59`) and the comments at the top of `00_buildings.txt`, which say
//! that a building with `province_max` in its `level_cap` is a provincial building, that `country_modifiers` take
//! country modifiers (optionally limited by `enable_for_controllers`) and `state_modifiers` take state modifiers.
//! Values are stored as written; what they do (costs, slots, damage) is decided by the systems that use them.
//! `dlc_allowed` (a trigger) and `missing_tech_loc` (bindable localisation) are deferred to the script VM.

use std::collections::BTreeMap;

use clausewitz::{Block, Entry};
use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Constants, Ctx, Fields, Source, evaluation_order};
use crate::types::{NumberList, Tag};

/// Numeric keys.
pub const BUILDING_NUMBERS: [&str; 18] = [
    "base_cost",
    "base_cost_conversion",
    "per_level_extra_cost",
    "per_controlled_building_extra_cost",
    "value",
    "icon_frame",
    "show_on_map",
    "show_on_map_meshes",
    "damage_factor",
    "repair_speed_factor",
    "military_production",
    "general_production",
    "naval_production",
    "land_fort",
    "naval_fort",
    "air_defence",
    "rocket_production",
    "rocket_launch_capacity",
];

/// Yes/no switches.
pub const BUILDING_FLAGS: [&str; 28] = [
    "is_buildable",
    "infrastructure",
    "air_base",
    "supply_node",
    "is_port",
    "anti_air",
    "refinery",
    "fuel_silo",
    "radar",
    "gun_emplacement",
    "naval_supply_hub",
    "naval_headquarter",
    "nuclear_reactor",
    "allied_build",
    "affects_energy",
    "infrastructure_construction_effect",
    "disabled_in_dmz",
    "only_costal",
    "need_supply",
    "need_detection",
    "show_modifier",
    "centered",
    "always_shown",
    "only_display_if_exists",
    "has_destroyed_mesh",
    "hide_if_missing_tech",
    "disable_grow_animation",
    "drawn_at_distance",
];

/// Single-name keys.
pub const BUILDING_TEXTS: [&str; 3] = ["spawn_point", "special_icon", "detecting_intel_type"];

/// Prefix of `local_resources_<resource> = <amount>` (comment in `00_buildings.txt`: "May use local_resources_ + any
/// resource name").
pub const LOCAL_RESOURCES_PREFIX: &str = "local_resources_";

/// `level_cap = { ... }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LevelCap {
    pub state_max: Option<u32>,
    pub province_max: Option<u32>,
    pub shares_slots: Option<bool>,
    pub group_by: Option<String>,
    pub exclusive_with: Vec<String>,
}

/// `country_modifiers = { enable_for_controllers = { ... } modifiers = { ... } }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CountryModifiers {
    /// Empty means every controller (comment in `00_buildings.txt`).
    pub enable_for_controllers: Vec<Tag>,
    pub modifiers: NumberList,
}

/// `building_prerequisite = { building_dependency = { building = X level = N } }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildingDependency {
    pub building: String,
    pub level: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BuildingDef {
    pub name: String,
    pub file: String,
    pub level_cap: LevelCap,
    pub numbers: NumberList,
    pub flags: Vec<(String, bool)>,
    pub texts: Vec<(String, String)>,
    pub tags: Vec<String>,
    pub specialization: Vec<String>,
    pub modules: Vec<String>,
    pub province_damage_modifiers: Vec<String>,
    pub state_damage_modifier: Vec<String>,
    /// `local_resources_<resource>`: resource name and amount.
    pub local_resources: NumberList,
    pub country_modifiers: Option<CountryModifiers>,
    pub state_modifiers: NumberList,
    pub prerequisites: Vec<BuildingDependency>,
}

impl BuildingDef {
    pub fn flag(&self, key: &str) -> Option<bool> {
        self.flags.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.texts.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn number(&self, key: &str) -> Option<common::Fixed> {
        self.numbers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    /// "if province_max is set, then building is provincial" (comment in `00_buildings.txt`).
    pub fn is_provincial(&self) -> bool {
        self.level_cap.province_max.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnKind {
    State,
    Province,
}

/// `spawn_points = { name = { type = state|province max = N ... } }`: where building models are placed on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnPoint {
    pub name: String,
    pub kind: SpawnKind,
    pub max: Option<u32>,
    pub only_costal: Option<bool>,
    pub disable_auto_nudging: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct Buildings {
    /// In definition order (the order of the original's building lists).
    pub list: Vec<BuildingDef>,
    pub spawn_points: BTreeMap<String, SpawnPoint>,
}

impl Buildings {
    pub fn get(&self, name: &str) -> Option<&BuildingDef> {
        self.list.iter().find(|b| b.name == name)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

const DIR: &str = "common/buildings";
const S: &str = "building";

pub fn load(vfs: &Vfs, report: &mut Report) -> Buildings {
    let mut out = Buildings::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

pub fn parse_file(src: &Source, report: &mut Report, out: &mut Buildings) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    let consts = Constants::take(&mut top, &mut ctx);
    for list in top.take_all("buildings") {
        let Some(block) = ctx.block(list, "buildings") else { continue };
        let mut fields = Fields::new(block, "buildings");
        for entry in fields.take_matching(|_| true) {
            let Some(body) = ctx.block(entry, S) else { continue };
            let def = parse_building(entry.key.text, body, &consts, &mut ctx);
            if out.get(&def.name).is_some() {
                ctx.issue(entry.span, S, IssueKind::Redefinition("first"));
            } else {
                out.list.push(def);
            }
        }
        fields.finish(&mut ctx);
    }
    for list in top.take_all("spawn_points") {
        let Some(block) = ctx.block(list, "spawn_points") else { continue };
        let mut fields = Fields::new(block, "spawn_points");
        for entry in fields.take_matching(|_| true) {
            let Some(body) = ctx.block(entry, "spawn_point") else { continue };
            let Some(point) = parse_spawn_point(entry, body, &mut ctx) else { continue };
            if out.spawn_points.contains_key(&point.name) {
                ctx.issue(entry.span, "spawn_point", IssueKind::Redefinition("first"));
            } else {
                out.spawn_points.insert(point.name.clone(), point);
            }
        }
        fields.finish(&mut ctx);
    }
    top.finish(&mut ctx);
}

fn parse_building(name: &str, block: &Block<'_>, consts: &Constants, ctx: &mut Ctx<'_, '_>) -> BuildingDef {
    let mut f = Fields::new(block, S);
    let mut b = BuildingDef { name: name.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    for key in BUILDING_NUMBERS {
        if let Some(entry) = f.take(key)
            && let Some(v) = consts.fixed(ctx, entry, &format!("{S}.{key}"))
        {
            b.numbers.push((key.to_string(), v));
        }
    }
    for key in BUILDING_FLAGS {
        if let Some(entry) = f.take(key)
            && let Some(v) = ctx.bool(entry, &format!("{S}.{key}"))
        {
            b.flags.push((key.to_string(), v));
        }
    }
    for key in BUILDING_TEXTS {
        if let Some(entry) = f.take(key)
            && let Some(v) = ctx.text(entry, &format!("{S}.{key}"))
        {
            b.texts.push((key.to_string(), v));
        }
    }
    let lists: [(&str, &mut Vec<String>); 5] = [
        ("tags", &mut b.tags),
        ("specialization", &mut b.specialization),
        ("modules", &mut b.modules),
        ("province_damage_modifiers", &mut b.province_damage_modifiers),
        ("state_damage_modifier", &mut b.state_damage_modifier),
    ];
    for (key, slot) in lists {
        for entry in f.take_all(key) {
            slot.extend(ctx.names(entry, &format!("{S}.{key}")));
        }
    }
    for entry in f.take_matching(|k| k.len() > LOCAL_RESOURCES_PREFIX.len() && k.starts_with(LOCAL_RESOURCES_PREFIX)) {
        if let Some(v) = consts.fixed(ctx, entry, &format!("{S}.{LOCAL_RESOURCES_PREFIX}<resource>")) {
            b.local_resources.push((entry.key.text[LOCAL_RESOURCES_PREFIX.len()..].to_string(), v));
        }
    }
    if let Some(e) = f.take("level_cap")
        && let Some(lb) = ctx.block(e, "building.level_cap")
    {
        b.level_cap = parse_level_cap(lb, ctx);
    }
    if let Some(e) = f.take("country_modifiers")
        && let Some(cb) = ctx.block(e, "building.country_modifiers")
    {
        b.country_modifiers = Some(parse_country_modifiers(cb, ctx));
    }
    if let Some(e) = f.take("state_modifiers")
        && let Some(sb) = ctx.block(e, "building.state_modifiers")
    {
        b.state_modifiers = ctx.numbers(sb, "building.state_modifiers");
    }
    for e in f.take_all("building_prerequisite") {
        let Some(pb) = ctx.block(e, "building.building_prerequisite") else { continue };
        let mut pf = Fields::new(pb, "building.building_prerequisite");
        for d in pf.take_all("building_dependency") {
            b.prerequisites.extend(parse_dependency(d, ctx));
        }
        pf.finish(ctx);
    }
    f.defer_matching(|k| k == "dlc_allowed" || k == "missing_tech_loc");
    f.finish(ctx);
    b
}

fn parse_level_cap(block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> LevelCap {
    const L: &str = "building.level_cap";
    let mut f = Fields::new(block, L);
    let mut cap = LevelCap::default();
    if let Some(e) = f.take("state_max") {
        cap.state_max = ctx.u32(e, "building.level_cap.state_max");
    }
    if let Some(e) = f.take("province_max") {
        cap.province_max = ctx.u32(e, "building.level_cap.province_max");
    }
    if let Some(e) = f.take("shares_slots") {
        cap.shares_slots = ctx.bool(e, "building.level_cap.shares_slots");
    }
    if let Some(e) = f.take("group_by") {
        cap.group_by = ctx.text(e, "building.level_cap.group_by");
    }
    for e in f.take_all("exclusive_with") {
        cap.exclusive_with.extend(ctx.names(e, "building.level_cap.exclusive_with"));
    }
    f.finish(ctx);
    cap
}

fn parse_country_modifiers(block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> CountryModifiers {
    const C: &str = "building.country_modifiers";
    let mut f = Fields::new(block, C);
    let mut out = CountryModifiers::default();
    for e in f.take_all("enable_for_controllers") {
        for name in ctx.names(e, "building.country_modifiers.enable_for_controllers") {
            match Tag::parse(&name) {
                Some(t) => out.enable_for_controllers.push(t),
                None => ctx.issue(
                    e.span,
                    "building.country_modifiers.enable_for_controllers",
                    IssueKind::InvalidValue("a country tag"),
                ),
            }
        }
    }
    if let Some(e) = f.take("modifiers")
        && let Some(mb) = ctx.block(e, "building.country_modifiers.modifiers")
    {
        out.modifiers = ctx.numbers(mb, "building.country_modifiers.modifiers");
    }
    f.finish(ctx);
    out
}

fn parse_dependency(entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>) -> Option<BuildingDependency> {
    const D: &str = "building.building_prerequisite.building_dependency";
    let block = ctx.block(entry, D)?;
    let mut f = Fields::new(block, D);
    let building = f.take("building").and_then(|e| ctx.text(e, &format!("{D}.building")));
    let level = f.take("level").and_then(|e| ctx.int(e, &format!("{D}.level")));
    f.finish(ctx);
    match (building, level) {
        (Some(building), Some(level)) => Some(BuildingDependency { building, level }),
        _ => {
            ctx.issue(entry.span, D, IssueKind::Missing("building and level"));
            None
        }
    }
}

fn parse_spawn_point(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> Option<SpawnPoint> {
    const P: &str = "spawn_point";
    let mut f = Fields::new(block, P);
    let kind = match f.take("type") {
        Some(e) => match ctx.text(e, "spawn_point.type").as_deref() {
            Some("state") => Some(SpawnKind::State),
            Some("province") => Some(SpawnKind::Province),
            Some(_) => {
                ctx.issue(e.span, "spawn_point.type", IssueKind::InvalidValue("state or province"));
                None
            }
            None => None,
        },
        None => {
            ctx.issue(entry.span, "spawn_point.type", IssueKind::Missing("type"));
            None
        }
    };
    let max = f.take("max").and_then(|e| ctx.u32(e, "spawn_point.max"));
    let only_costal = f.take("only_costal").and_then(|e| ctx.bool(e, "spawn_point.only_costal"));
    let disable_auto_nudging =
        f.take("disable_auto_nudging").and_then(|e| ctx.bool(e, "spawn_point.disable_auto_nudging"));
    f.finish(ctx);
    Some(SpawnPoint { name: entry.key.text.to_string(), kind: kind?, max, only_costal, disable_auto_nudging })
}
