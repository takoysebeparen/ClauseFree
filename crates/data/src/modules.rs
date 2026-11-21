//! Equipment modules: `common/units/equipment/modules/*.txt`, blocks `equipment_modules`.
//!
//! Source: the files themselves and a key inventory of them (revision `8873ec77`, game 1.20.0.0). The equipment
//! syntax reference (`common/units/equipment/_documentation.md`) documents the equipment side (slots, count limits,
//! default modules) but not the module blocks, so the shapes here come from the files only. Values are stored as
//! written; how `add_stats`, `multiply_stats` and `add_average_stats` combine into a design's stats is decided by the
//! production system and must be measured (see `docs/CANON.md`, "Equipment modules").
//!
//! A module category is not declared anywhere: it exists because a module names it in `category`.

use std::collections::{BTreeMap, BTreeSet};

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::NumberList;

/// Numeric keys of a module.
pub const MODULE_NUMBERS: [&str; 3] = ["xp_cost", "manpower", "dismantle_cost_ic"];

/// Yes/no switches of a module.
pub const MODULE_FLAGS: [&str; 1] = ["mega_carrier"];

/// Single-name keys of a module.
pub const MODULE_TEXTS: [&str; 5] = ["abbreviation", "sfx", "gfx", "gui_category", "parent"];

/// Name lists of a module (one name or `{ a b }`).
pub const MODULE_LISTS: [&str; 7] = [
    "critical_parts",
    "allow_equipment_type",
    "forbid_equipment_type",
    "forbid_equipment_type_exact_match",
    "add_equipment_type",
    "allow_mission_type",
    "forbid_module_categories",
];

/// `add_stats`, `multiply_stats` and `add_average_stats`, as written.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatChanges {
    pub add: NumberList,
    pub multiply: NumberList,
    pub average: NumberList,
}

impl StatChanges {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.multiply.is_empty() && self.average.is_empty()
    }
}

/// `mission_type_stats = { limit = { <mission> ... } add_stats = { ... } ... }`: stats that apply only on the listed
/// air missions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MissionStats {
    pub missions: Vec<String>,
    pub stats: StatChanges,
}

/// What a module can be converted from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertFrom {
    Module(String),
    Category(String),
}

/// `can_convert_from = { module = X | module_category = Y convert_cost_ic = N convert_cost_resources = { ... } }`.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversion {
    pub from: ConvertFrom,
    pub cost_ic: Option<Fixed>,
    pub cost_resources: NumberList,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModuleDef {
    pub name: String,
    pub file: String,
    pub category: String,
    /// The surrounding `equipment_modules` block has a `limit` (a trigger such as `has_dlc`); whether the module
    /// exists in a game is decided by the script VM.
    pub conditional: bool,
    pub numbers: NumberList,
    pub flags: Vec<(String, bool)>,
    pub texts: Vec<(String, String)>,
    /// The keys of [`MODULE_LISTS`] that are present, with their names.
    pub lists: Vec<(String, Vec<String>)>,
    pub stats: StatChanges,
    pub mission_stats: Vec<MissionStats>,
    pub build_cost_resources: NumberList,
    pub dismantle_cost_resources: NumberList,
    pub conversions: Vec<Conversion>,
    /// `forbid_equipment_type_exact_match_for_category = { <module category> = <equipment type> }`.
    pub forbid_exact_match_for_category: Vec<(String, String)>,
    /// `allowed_module_categories = { <slot> = { <category> ... } }`: what a module (a turret) allows in other slots.
    pub allowed_module_categories: Vec<(String, Vec<String>)>,
}

impl ModuleDef {
    pub fn flag(&self, key: &str) -> Option<bool> {
        self.flags.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.texts.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn number(&self, key: &str) -> Option<Fixed> {
        self.numbers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    pub fn list(&self, key: &str) -> &[String] {
        self.lists.iter().find(|(k, _)| k == key).map_or(&[], |(_, v)| v.as_slice())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Modules {
    /// In definition order (the order of the designer's module lists).
    pub list: Vec<ModuleDef>,
    index: BTreeMap<String, usize>,
}

impl Modules {
    pub fn get(&self, name: &str) -> Option<&ModuleDef> {
        self.index.get(name).and_then(|&i| self.list.get(i))
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Every module category, i.e. every name some module gives as its `category`.
    pub fn categories(&self) -> BTreeSet<&str> {
        self.list.iter().map(|m| m.category.as_str()).filter(|c| !c.is_empty()).collect()
    }

    fn insert(&mut self, def: ModuleDef) {
        self.index.insert(def.name.clone(), self.list.len());
        self.list.push(def);
    }
}

const DIR: &str = "common/units/equipment/modules";
const S: &str = "module";

pub fn load(vfs: &Vfs, report: &mut Report) -> Modules {
    let mut out = Modules::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

pub fn parse_file(src: &Source, report: &mut Report, out: &mut Modules) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for list in top.take_all("equipment_modules") {
        let Some(block) = ctx.block(list, "equipment_modules") else { continue };
        let mut fields = Fields::new(block, "equipment_modules");
        // A trigger: understood, evaluated by the script VM.
        fields.defer_matching(|k| k == "limit");
        let conditional = block.entries().any(|e| e.key.text == "limit");
        for entry in fields.take_matching(|k| k != "limit") {
            let Some(body) = ctx.block(entry, S) else { continue };
            let def = parse_module(entry, body, conditional, &mut ctx);
            if out.get(&def.name).is_some() {
                ctx.issue(entry.span, S, IssueKind::Redefinition("first"));
            } else {
                out.insert(def);
            }
        }
        fields.finish(&mut ctx);
    }
    top.finish(&mut ctx);
}

fn parse_module(entry: &Entry<'_>, block: &Block<'_>, conditional: bool, ctx: &mut Ctx<'_, '_>) -> ModuleDef {
    let mut f = Fields::new(block, S);
    let mut m =
        ModuleDef { name: entry.key.text.to_string(), file: ctx.src.path.clone(), conditional, ..Default::default() };
    match f.take("category") {
        Some(e) => m.category = ctx.text(e, "module.category").unwrap_or_default(),
        None => ctx.issue(entry.span, "module.category", IssueKind::Missing("category")),
    }
    for key in MODULE_NUMBERS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.fixed(e, &format!("{S}.{key}"))
        {
            m.numbers.push((key.to_string(), v));
        }
    }
    for key in MODULE_FLAGS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.bool(e, &format!("{S}.{key}"))
        {
            m.flags.push((key.to_string(), v));
        }
    }
    for key in MODULE_TEXTS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.text(e, &format!("{S}.{key}"))
        {
            m.texts.push((key.to_string(), v));
        }
    }
    for key in MODULE_LISTS {
        let entries = f.take_all(key);
        if entries.is_empty() {
            continue;
        }
        let mut names = Vec::new();
        for e in entries {
            names.extend(ctx.names(e, &format!("{S}.{key}")));
        }
        m.lists.push((key.to_string(), names));
    }
    m.stats = stat_changes(&mut f, ctx, S);
    for e in f.take_all("mission_type_stats") {
        let Some(b) = ctx.block(e, "module.mission_type_stats") else { continue };
        let mut mf = Fields::new(b, "module.mission_type_stats");
        let mut ms = MissionStats::default();
        for l in mf.take_all("limit") {
            ms.missions.extend(ctx.names(l, "module.mission_type_stats.limit"));
        }
        ms.stats = stat_changes(&mut mf, ctx, "module.mission_type_stats");
        mf.finish(ctx);
        m.mission_stats.push(ms);
    }
    for (key, slot) in [
        ("build_cost_resources", &mut m.build_cost_resources),
        ("dismantle_cost_resources", &mut m.dismantle_cost_resources),
    ] {
        if let Some(e) = f.take(key)
            && let Some(b) = ctx.block(e, &format!("{S}.{key}"))
        {
            *slot = ctx.numbers(b, &format!("{S}.{key}"));
        }
    }
    for e in f.take_all("can_convert_from") {
        m.conversions.extend(parse_conversion(e, ctx));
    }
    for e in f.take_all("forbid_equipment_type_exact_match_for_category") {
        const P: &str = "module.forbid_equipment_type_exact_match_for_category";
        let Some(b) = ctx.block(e, P) else { continue };
        let mut pf = Fields::new(b, P);
        for pair in pf.take_matching(|_| true) {
            if let Some(v) = ctx.text(pair, &format!("{P}.<category>")) {
                m.forbid_exact_match_for_category.push((pair.key.text.to_string(), v));
            }
        }
        pf.finish(ctx);
    }
    for e in f.take_all("allowed_module_categories") {
        const P: &str = "module.allowed_module_categories";
        let Some(b) = ctx.block(e, P) else { continue };
        let mut sf = Fields::new(b, P);
        for slot in sf.take_matching(|_| true) {
            m.allowed_module_categories.push((slot.key.text.to_string(), ctx.names(slot, &format!("{P}.<slot>"))));
        }
        sf.finish(ctx);
    }
    f.finish(ctx);
    m
}

/// Takes `add_stats`, `multiply_stats` and `add_average_stats` from a block. Stat names are not checked here: no
/// file lists the valid ones, and modules use names (`thrust`, `naval_torpedo_hit_chance_factor`, ...) that no
/// equipment sets directly.
fn stat_changes(f: &mut Fields<'_, '_>, ctx: &mut Ctx<'_, '_>, schema: &str) -> StatChanges {
    let mut out = StatChanges::default();
    for (key, slot) in
        [("add_stats", &mut out.add), ("multiply_stats", &mut out.multiply), ("add_average_stats", &mut out.average)]
    {
        for e in f.take_all(key) {
            if let Some(b) = ctx.block(e, &format!("{schema}.{key}")) {
                slot.extend(ctx.numbers(b, &format!("{schema}.{key}")));
            }
        }
    }
    out
}

fn parse_conversion(entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>) -> Option<Conversion> {
    const P: &str = "module.can_convert_from";
    let block = ctx.block(entry, P)?;
    // `can_convert_from = { }` occurs in the base game (`00_ship_modules.txt`) and says nothing.
    if block.items.is_empty() {
        return None;
    }
    let mut f = Fields::new(block, P);
    let module = f.take("module").and_then(|e| ctx.text(e, "module.can_convert_from.module"));
    let category = f.take("module_category").and_then(|e| ctx.text(e, "module.can_convert_from.module_category"));
    let cost_ic = f.take("convert_cost_ic").and_then(|e| ctx.fixed(e, "module.can_convert_from.convert_cost_ic"));
    let mut cost_resources = NumberList::new();
    if let Some(e) = f.take("convert_cost_resources")
        && let Some(b) = ctx.block(e, "module.can_convert_from.convert_cost_resources")
    {
        cost_resources = ctx.numbers(b, "module.can_convert_from.convert_cost_resources");
    }
    f.finish(ctx);
    let from = match (module, category) {
        (Some(m), None) => ConvertFrom::Module(m),
        (None, Some(c)) => ConvertFrom::Category(c),
        _ => {
            ctx.issue(entry.span, P, IssueKind::InvalidValue("exactly one of module and module_category"));
            return None;
        }
    };
    Some(Conversion { from, cost_ic, cost_resources })
}
