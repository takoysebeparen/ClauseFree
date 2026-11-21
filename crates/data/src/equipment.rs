//! Equipment: `common/units/equipment/*.txt`, blocks `equipments` and `duplicate_archetypes`.
//!
//! Source: the syntax reference shipped with the game, `common/units/equipment/_documentation.md`, plus a key
//! inventory of the files (revisions `7c6c1c59` and `8873ec77`). Stats are stored as written. Modular equipment
//! (`module_slots`, `module_count_limit`, `default_modules`, `module_add_stats_multiplier`) follows the same
//! reference; the modules themselves are in [`crate::modules`].

use std::collections::BTreeMap;

use clausewitz::{Block, Entry, Operator, Value};
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

/// Keys that hold triggers (script) or data for later layers.
const DEFERRED: [&str; 7] = [
    "can_be_produced",
    "can_be_lend_leased",
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
    /// `module_slots`; `None` if the key is absent.
    pub module_slots: Option<ModuleSlots>,
    pub module_count_limits: Vec<ModuleCountLimit>,
    /// `default_modules = { <slot> = <module>|empty }`, as written.
    pub default_modules: Vec<(String, String)>,
    pub module_add_stats_multiplier: NumberList,
}

/// `module_slots = ...`.
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleSlots {
    /// `module_slots = inherit`: every slot of the parent equipment type.
    Inherit,
    /// `module_slots = none`. Comment in `x_plane_airframes.txt`: "Prevent this archetype from being identified as
    /// using the plane designer".
    None,
    List(Vec<ModuleSlot>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleSlot {
    pub name: String,
    pub spec: SlotSpec,
}

impl ModuleSlot {
    /// "Fixed slots ... always start with `fixed_`" (documentation).
    pub fn is_fixed(&self) -> bool {
        self.name.starts_with("fixed_")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SlotSpec {
    Defined {
        required: Option<bool>,
        allowed_categories: Vec<String>,
        gfx: Option<String>,
    },
    /// `<slot> = inherit`: the same slot of the parent equipment type.
    Inherit,
    /// `<slot> = <earlier slot>`: the attributes of an earlier slot of the same block.
    SameAs(String),
}

/// One match condition of a `module_count_limit`. The names `any` (module or category) and `empty` (module) are
/// kept as written; the documentation gives them special meanings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitMatch {
    Module(String),
    Category(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountRule {
    /// `count < N`
    Below(u32),
    /// `count > N`
    Above(u32),
    /// `count = N`
    Exactly(u32),
    /// `count = any`
    Any,
}

/// `module_count_limit = { module = X category = Y count < N title = T }`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleCountLimit {
    pub matches: Vec<LimitMatch>,
    pub count: CountRule,
    pub title: Option<String>,
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
    /// - list-valued attributes (`type`, `upgrades`, `resources`, `default_modules`, ...) are replaced as a whole,
    ///   not merged — an assumption;
    /// - module slots: `module_slots = inherit`, a slot written `inherit` and a default module written `inherit`
    ///   take their value from "the parent equipment type" (documentation): the `parent` equipment if there is one,
    ///   else the archetype. The base game confirms `parent`: `heavy_cruiser_3` (`parent = heavy_cruiser_2`,
    ///   `module_slots = inherit`) puts default modules in `rear_2_custom_slot`, which only `heavy_cruiser_1`
    ///   defines, not the archetype. An absent `module_slots` takes the archetype's (general rule above, an
    ///   assumption). `<slot> = <earlier slot>` copies an earlier slot. The result holds only
    ///   [`SlotSpec::Defined`] slots;
    /// - module count limits are inherited from the archetype, and "an inherited limit is overridden if the child
    ///   has a limit with exactly the same match conditions" (documentation).
    ///
    /// `None` if the name is unknown, a chain is broken or circular, or an `inherit` cannot be resolved.
    pub fn effective(&self, name: &str) -> Option<EquipmentDef> {
        self.resolve(name, 0)
    }

    fn resolve(&self, name: &str, depth: usize) -> Option<EquipmentDef> {
        // The base game's chains are at most a few steps long; anything deeper is a loop.
        if depth > 32 {
            return None;
        }
        let own = self.get(name)?;
        let base = match own.text("archetype") {
            Some(a) => Some(self.resolve(a, depth + 1)?),
            None => None,
        };
        let mut out = own.clone();
        if let Some(base) = &base {
            inherit_from_archetype(&mut out, own, base);
        }
        // `inherit` in `module_slots` and `default_modules` refers to "the parent equipment type": `parent` if the
        // equipment has one, else its archetype.
        let needs_parent = matches!(own.module_slots, Some(ModuleSlots::Inherit))
            || matches!(&own.module_slots, Some(ModuleSlots::List(l)) if l.iter().any(|s| s.spec == SlotSpec::Inherit))
            || own.default_modules.iter().any(|(_, m)| m == INHERIT);
        let parent = match own.text("parent") {
            Some(p) if needs_parent => Some(self.resolve(p, depth + 1)?),
            _ => None,
        };
        let source = parent.as_ref().or(base.as_ref());
        out.module_slots = match &own.module_slots {
            None => base.as_ref().and_then(|b| b.module_slots.clone()),
            Some(own_slots) => resolve_slots(own_slots, source.and_then(|s| s.module_slots.as_ref()))?,
        };
        for (slot, module) in &mut out.default_modules {
            if module == INHERIT {
                let inherited = source?.default_modules.iter().find(|(s, _)| s == slot)?;
                module.clone_from(&inherited.1);
            }
        }
        Some(out)
    }
}

/// `inherit` as a value of `module_slots`, of a slot and of a default module.
const INHERIT: &str = "inherit";

/// Fills in what `out` (a copy of `own`) inherits from its resolved archetype `base`.
fn inherit_from_archetype(out: &mut EquipmentDef, own: &EquipmentDef, base: &EquipmentDef) {
    let mut base = base.clone();
    base.flags.retain(|(k, _)| k != "is_archetype" && k != "is_buildable");
    if own.duplicate {
        base.texts.retain(|(k, _)| k != "alias" && k != "derived_variant_name");
        base.types.clear();
    }
    merge_pairs(&mut out.numbers, &base.numbers);
    merge_pairs(&mut out.flags, &base.flags);
    merge_pairs(&mut out.texts, &base.texts);
    merge_pairs(&mut out.module_add_stats_multiplier, &base.module_add_stats_multiplier);
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
    if out.default_modules.is_empty() {
        out.default_modules.clone_from(&base.default_modules);
    }
    for limit in &base.module_count_limits {
        if !own.module_count_limits.iter().any(|l| same_conditions(&l.matches, &limit.matches)) {
            out.module_count_limits.push(limit.clone());
        }
    }
}

/// The slots of an equipment from its own `module_slots` and the resolved slots of the equipment `inherit` refers
/// to: `Some(None)` if there are no slots (`inherit` from an equipment without slots), `None` if a slot refers to
/// something that does not exist.
fn resolve_slots(own: &ModuleSlots, inherited: Option<&ModuleSlots>) -> Option<Option<ModuleSlots>> {
    let inherited_slots: &[ModuleSlot] = match inherited {
        Some(ModuleSlots::List(l)) => l,
        _ => &[],
    };
    let list = match own {
        ModuleSlots::Inherit => return Some(inherited.cloned()),
        ModuleSlots::None => return Some(Some(ModuleSlots::None)),
        ModuleSlots::List(l) => l,
    };
    let mut out: Vec<ModuleSlot> = Vec::with_capacity(list.len());
    for slot in list {
        let spec = match &slot.spec {
            SlotSpec::Defined { .. } => slot.spec.clone(),
            SlotSpec::Inherit => inherited_slots.iter().find(|b| b.name == slot.name)?.spec.clone(),
            SlotSpec::SameAs(earlier) => out.iter().find(|o| &o.name == earlier)?.spec.clone(),
        };
        out.push(ModuleSlot { name: slot.name.clone(), spec });
    }
    Some(Some(ModuleSlots::List(out)))
}

/// "Exactly the same match conditions": the same set of conditions, in any order (order is an assumption).
fn same_conditions(a: &[LimitMatch], b: &[LimitMatch]) -> bool {
    a.len() == b.len() && a.iter().all(|m| b.contains(m))
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
    if let Some(entry) = f.take("module_slots") {
        e.module_slots = parse_module_slots(entry, ctx);
    }
    for entry in f.take_all("module_count_limit") {
        e.module_count_limits.extend(parse_count_limit(entry, ctx));
    }
    for entry in f.take_all("default_modules") {
        const P: &str = "equipment.default_modules";
        let Some(b) = ctx.block(entry, P) else { continue };
        let mut df = Fields::new(b, P);
        for slot in df.take_matching(|_| true) {
            if let Some(m) = ctx.text(slot, "equipment.default_modules.<slot>") {
                e.default_modules.push((slot.key.text.to_string(), m));
            }
        }
        df.finish(ctx);
    }
    if let Some(entry) = f.take("module_add_stats_multiplier")
        && let Some(b) = ctx.block(entry, "equipment.module_add_stats_multiplier")
    {
        e.module_add_stats_multiplier = ctx.numbers(b, "equipment.module_add_stats_multiplier");
    }
    f.defer_matching(|k| DEFERRED.contains(&k));
    f.finish(ctx);
    e
}

fn parse_module_slots(entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>) -> Option<ModuleSlots> {
    const P: &str = "equipment.module_slots";
    const SLOT: &str = "equipment.module_slots.<slot>";
    if let Value::Scalar(s) = &entry.value {
        return match s.text {
            "inherit" => Some(ModuleSlots::Inherit),
            "none" => Some(ModuleSlots::None),
            _ => {
                ctx.issue(s.span, P, IssueKind::InvalidValue("inherit, none or a block"));
                None
            }
        };
    }
    let block = ctx.block(entry, P)?;
    let mut f = Fields::new(block, P);
    let mut out: Vec<ModuleSlot> = Vec::new();
    for slot in f.take_matching(|_| true) {
        let name = slot.key.text.to_string();
        if out.iter().any(|o| o.name == name) {
            ctx.issue(slot.span, SLOT, IssueKind::Redefinition("first"));
            continue;
        }
        let spec = match &slot.value {
            Value::Scalar(s) if s.text == "inherit" => SlotSpec::Inherit,
            Value::Scalar(s) => {
                if !out.iter().any(|o| o.name == s.text) {
                    ctx.issue(s.span, SLOT, IssueKind::BrokenReference("earlier slot"));
                    continue;
                }
                SlotSpec::SameAs(s.text.to_string())
            }
            _ => {
                let Some(b) = ctx.block(slot, SLOT) else { continue };
                parse_slot(b, ctx)
            }
        };
        out.push(ModuleSlot { name, spec });
    }
    f.finish(ctx);
    Some(ModuleSlots::List(out))
}

fn parse_slot(block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> SlotSpec {
    let mut f = Fields::new(block, "equipment.module_slots.<slot>");
    let required = f.take("required").and_then(|e| ctx.bool(e, "equipment.module_slots.<slot>.required"));
    let mut allowed_categories = Vec::new();
    for e in f.take_all("allowed_module_categories") {
        allowed_categories.extend(ctx.names(e, "equipment.module_slots.<slot>.allowed_module_categories"));
    }
    let gfx = f.take("gfx").and_then(|e| ctx.text(e, "equipment.module_slots.<slot>.gfx"));
    f.finish(ctx);
    SlotSpec::Defined { required, allowed_categories, gfx }
}

fn parse_count_limit(entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>) -> Option<ModuleCountLimit> {
    const P: &str = "equipment.module_count_limit";
    let block = ctx.block(entry, P)?;
    let mut f = Fields::new(block, P);
    let mut matches = Vec::new();
    for e in f.take_all("module") {
        matches.extend(ctx.text(e, "equipment.module_count_limit.module").map(LimitMatch::Module));
    }
    for e in f.take_all("category") {
        matches.extend(ctx.text(e, "equipment.module_count_limit.category").map(LimitMatch::Category));
    }
    let title = f.take("title").and_then(|e| ctx.text(e, "equipment.module_count_limit.title"));
    let count = f.take("count").and_then(|e| parse_count(e, ctx));
    f.finish(ctx);
    if matches.is_empty() {
        ctx.issue(entry.span, P, IssueKind::Missing("module or category"));
        return None;
    }
    let Some(count) = count else {
        ctx.issue(entry.span, P, IssueKind::Missing("count"));
        return None;
    };
    Some(ModuleCountLimit { matches, count, title })
}

fn parse_count(entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>) -> Option<CountRule> {
    const P: &str = "equipment.module_count_limit.count";
    if entry.op == Operator::Eq && entry.value.as_scalar().is_some_and(|s| s.text == "any") {
        return Some(CountRule::Any);
    }
    let rule: fn(u32) -> CountRule = match entry.op {
        Operator::Lt => CountRule::Below,
        Operator::Gt => CountRule::Above,
        Operator::Eq => CountRule::Exactly,
        _ => {
            ctx.issue(entry.span, P, IssueKind::InvalidValue("count < N, count > N, count = N or count = any"));
            return None;
        }
    };
    ctx.u32(entry, P).map(rule)
}
