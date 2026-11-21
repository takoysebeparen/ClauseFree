//! Technologies: `common/technologies/*.txt` (block `technologies`) and the tags they use,
//! `common/technology_tags/*.txt` (`technology_categories`, `technology_folders`).
//!
//! Source: the files themselves and a key inventory of them (revision `8873ec77`, game 1.20.0.0). No shipped
//! documentation covers technologies, so the shapes come from the files only. Values are stored as written; what they
//! do (research time, unlocks, bonuses) is decided by the research and combat systems (see `docs/CANON.md`,
//! "Technologies").
//!
//! Besides the keys listed here, a technology holds two kinds of open-ended entries:
//! - `name = number`: a country modifier (`research_speed_factor = 0.03`);
//! - `name = { ... }`: stats for a sub-unit or sub-unit category (`infantry = { soft_attack = 0.05 }`,
//!   `category_light_infantry = { forest = { attack = 0.05 } }`), see [`UnitStats`].
//!
//! Doctrines live in `common/doctrines` since 1.17 (a separate system, see [`crate::doctrines`]), even though the
//! doctrine folders are still declared in `technology_folders`.

use std::collections::BTreeMap;

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Constants, Ctx, Fields, Source, evaluation_order};
use crate::types::NumberList;
use crate::units::{BattalionMult, parse_battalion_mult};

/// Numeric keys of a technology (besides `research_cost` and `start_year`, which have their own fields).
pub const TECH_NUMBERS: [&str; 3] = ["xp_boost_cost", "xp_research_bonus", "sub_tech_index"];

/// Yes/no switches of a technology.
pub const TECH_FLAGS: [&str; 4] =
    ["is_special_project_tech", "show_effect_as_desc", "force_use_small_tech_layout", "show_equipment_icon"];

/// Single-name keys of a technology.
pub const TECH_TEXTS: [&str; 2] = ["xp_research_type", "desc"];

/// Triggers, effects and AI weights: understood, evaluated by the script VM.
pub const TECH_SCRIPTS: [&str; 5] =
    ["allow", "allow_branch", "ai_will_do", "on_research_complete", "on_research_complete_limit"];

/// `folder = { name = X position = { x = N y = N } }`: where the technology sits in a research screen tab.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderPosition {
    pub folder: String,
    pub x: Fixed,
    pub y: Fixed,
}

/// `path = { leads_to_tech = X research_cost_coeff = N ignore_for_layout = yes }`: an arrow to a follow-up technology.
#[derive(Debug, Clone, PartialEq)]
pub struct TechPath {
    pub leads_to: String,
    pub research_cost_coeff: Option<Fixed>,
    pub ignore_for_layout: bool,
}

/// One `enable_equipments`, `enable_equipment_modules` or `enable_subunits` block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Unlock {
    pub names: Vec<String>,
    /// The block has a `limit` (a trigger such as `has_dlc`); whether the unlock happens is decided by the script VM.
    pub conditional: bool,
}

/// `<sub-unit or sub-unit category> = { <stat> = N <terrain> = { ... } battalion_mult = { ... } }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UnitStats {
    /// A sub-unit (`infantry`) or a sub-unit category (`category_light_infantry`); checked by the database.
    pub target: String,
    pub stats: NumberList,
    /// Per-terrain (or per-situation, see [`crate::units::COMBAT_SITUATIONS`]) modifiers.
    pub terrain: BTreeMap<String, NumberList>,
    pub battalion_mult: Vec<BattalionMult>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TechDef {
    pub name: String,
    pub file: String,
    pub research_cost: Option<Fixed>,
    pub start_year: Option<i64>,
    pub folders: Vec<FolderPosition>,
    pub categories: Vec<String>,
    pub paths: Vec<TechPath>,
    /// `XOR = { ... }` (also written `xor`): technologies that exclude this one.
    pub xor: Vec<String>,
    /// `dependencies = { <tech> = N }`: required technologies, as written.
    pub dependencies: NumberList,
    pub sub_technologies: Vec<String>,
    pub enable_equipments: Vec<Unlock>,
    pub enable_equipment_modules: Vec<Unlock>,
    pub enable_subunits: Vec<Unlock>,
    /// `enable_building = { building = X level = N }`.
    pub enable_buildings: Vec<(String, i64)>,
    pub special_project_specialization: Vec<String>,
    pub ai_research_weights: NumberList,
    pub numbers: NumberList,
    pub flags: Vec<(String, bool)>,
    pub texts: Vec<(String, String)>,
    /// Country modifiers: every other `name = number` entry. Names are not checked here.
    pub modifiers: NumberList,
    pub unit_stats: Vec<UnitStats>,
}

impl TechDef {
    pub fn flag(&self, key: &str) -> Option<bool> {
        self.flags.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.texts.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn number(&self, key: &str) -> Option<Fixed> {
        self.numbers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    pub fn modifier(&self, key: &str) -> Option<Fixed> {
        self.modifiers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    pub fn unit_stats(&self, target: &str) -> Option<&UnitStats> {
        self.unit_stats.iter().find(|u| u.target == target)
    }

    /// Every equipment named by an `enable_equipments` block, conditional or not.
    pub fn enabled_equipment(&self) -> impl Iterator<Item = &str> {
        self.enable_equipments.iter().flat_map(|u| u.names.iter().map(String::as_str))
    }
}

/// `technology_folders = { <name> = { ledger = X doctrine = yes available = { ... } } }`: a research screen tab.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TechFolder {
    pub name: String,
    pub ledger: Option<String>,
    pub doctrine: bool,
    /// The folder has an `available` trigger (deferred to the script VM).
    pub conditional: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Technologies {
    /// In definition order.
    pub list: Vec<TechDef>,
    index: BTreeMap<String, usize>,
    /// `technology_categories`, in definition order without repeats.
    pub categories: Vec<String>,
    /// `technology_folders`, in definition order.
    pub folders: Vec<TechFolder>,
}

impl Technologies {
    pub fn get(&self, name: &str) -> Option<&TechDef> {
        self.index.get(name).and_then(|&i| self.list.get(i))
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn has_category(&self, name: &str) -> bool {
        self.categories.iter().any(|c| c == name)
    }

    pub fn folder(&self, name: &str) -> Option<&TechFolder> {
        self.folders.iter().find(|f| f.name == name)
    }

    fn insert(&mut self, def: TechDef) {
        self.index.insert(def.name.clone(), self.list.len());
        self.list.push(def);
    }
}

const DIR: &str = "common/technologies";
const TAGS_DIR: &str = "common/technology_tags";
const S: &str = "technology";

pub fn load(vfs: &Vfs, report: &mut Report) -> Technologies {
    let mut out = Technologies::default();
    for file in evaluation_order(vfs.list(TAGS_DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_tags(&src, report, &mut out);
    }
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

/// Reads `technology_categories` and `technology_folders`. A repeated category or folder is reported and the first
/// kept.
pub fn parse_tags(src: &Source, report: &mut Report, out: &mut Technologies) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for e in top.take_all("technology_categories") {
        let Some(b) = ctx.block(e, "technology_categories") else { continue };
        for v in b.values() {
            match v.as_scalar() {
                Some(s) if out.has_category(s.text) => {
                    ctx.issue(v.span(), "technology_categories", IssueKind::Redefinition("first"))
                }
                Some(s) => out.categories.push(s.unescaped().into_owned()),
                None => ctx.issue(v.span(), "technology_categories", IssueKind::InvalidValue("a list of names")),
            }
        }
        for entry in b.entries() {
            ctx.issue(entry.span, "technology_categories", IssueKind::InvalidValue("a list of names"));
        }
    }
    for e in top.take_all("technology_folders") {
        const P: &str = "technology_folders";
        let Some(b) = ctx.block(e, P) else { continue };
        let mut ff = Fields::new(b, P);
        for entry in ff.take_matching(|_| true) {
            let Some(body) = ctx.block(entry, "technology_folder") else { continue };
            let mut f = Fields::new(body, "technology_folder");
            let mut folder = TechFolder { name: entry.key.text.to_string(), ..Default::default() };
            if let Some(l) = f.take("ledger") {
                folder.ledger = ctx.text(l, "technology_folder.ledger");
            }
            if let Some(d) = f.take("doctrine") {
                folder.doctrine = ctx.bool(d, "technology_folder.doctrine").unwrap_or(false);
            }
            folder.conditional = body.entries().any(|e| e.key.text == "available");
            f.defer_matching(|k| k == "available");
            f.finish(&mut ctx);
            if out.folder(&folder.name).is_some() {
                ctx.issue(entry.span, "technology_folder", IssueKind::Redefinition("first"));
            } else {
                out.folders.push(folder);
            }
        }
        ff.finish(&mut ctx);
    }
    top.finish(&mut ctx);
}

/// Reads one `common/technologies` file. `@name = value` constants inside the `technologies` block are local to the
/// file (see `docs/CANON.md`, "Script constants").
pub fn parse_file(src: &Source, report: &mut Report, out: &mut Technologies) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for list in top.take_all("technologies") {
        let Some(block) = ctx.block(list, "technologies") else { continue };
        let mut fields = Fields::new(block, "technologies");
        let constants = Constants::take(&mut fields, &mut ctx);
        for entry in fields.take_matching(|_| true) {
            let Some(body) = ctx.block(entry, S) else { continue };
            let def = parse_tech(entry, body, &constants, &mut ctx);
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

fn parse_tech(entry: &Entry<'_>, block: &Block<'_>, constants: &Constants, ctx: &mut Ctx<'_, '_>) -> TechDef {
    let mut f = Fields::new(block, S);
    let mut t = TechDef { name: entry.key.text.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    f.defer_matching(|k| TECH_SCRIPTS.contains(&k));
    if let Some(e) = f.take("research_cost") {
        t.research_cost = constants.fixed(ctx, e, "technology.research_cost");
    }
    if let Some(e) = f.take("start_year") {
        t.start_year = ctx.int(e, "technology.start_year");
    }
    for key in TECH_NUMBERS {
        if let Some(e) = f.take(key)
            && let Some(v) = constants.fixed(ctx, e, &format!("{S}.{key}"))
        {
            t.numbers.push((key.to_string(), v));
        }
    }
    for key in TECH_FLAGS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.bool(e, &format!("{S}.{key}"))
        {
            t.flags.push((key.to_string(), v));
        }
    }
    for key in TECH_TEXTS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.text(e, &format!("{S}.{key}"))
        {
            t.texts.push((key.to_string(), v));
        }
    }
    for e in f.take_all("folder") {
        t.folders.extend(parse_folder(e, constants, ctx));
    }
    for e in f.take_all("path") {
        t.paths.extend(parse_path(e, constants, ctx));
    }
    for (key, slot) in [
        ("categories", &mut t.categories),
        ("sub_technologies", &mut t.sub_technologies),
        ("special_project_specialization", &mut t.special_project_specialization),
    ] {
        for e in f.take_all(key) {
            slot.extend(ctx.names(e, &format!("{S}.{key}")));
        }
    }
    // Both spellings occur in the base game (`XOR` in industry.txt, `xor` in infantry.txt).
    for e in f.take_entries(|e| e.key.text.eq_ignore_ascii_case("xor")) {
        t.xor.extend(ctx.names(e, "technology.XOR"));
    }
    for (key, slot) in [("dependencies", &mut t.dependencies), ("ai_research_weights", &mut t.ai_research_weights)] {
        for e in f.take_all(key) {
            if let Some(b) = ctx.block(e, &format!("{S}.{key}")) {
                slot.extend(ctx.numbers(b, &format!("{S}.{key}")));
            }
        }
    }
    for (key, slot) in [
        ("enable_equipments", &mut t.enable_equipments),
        ("enable_equipment_modules", &mut t.enable_equipment_modules),
        ("enable_subunits", &mut t.enable_subunits),
    ] {
        for e in f.take_all(key) {
            slot.extend(parse_unlock(e, ctx, &format!("{S}.{key}")));
        }
    }
    for e in f.take_all("enable_building") {
        const P: &str = "technology.enable_building";
        let Some(b) = ctx.block(e, P) else { continue };
        let mut bf = Fields::new(b, P);
        let building = bf.take("building").and_then(|v| ctx.text(v, "technology.enable_building.building"));
        let level = bf.take("level").and_then(|v| ctx.int(v, "technology.enable_building.level"));
        bf.finish(ctx);
        match (building, level) {
            (Some(b), Some(l)) => t.enable_buildings.push((b, l)),
            _ => ctx.issue(e.span, P, IssueKind::Missing("building and level")),
        }
    }
    // What is left: blocks are unit stats, numbers are country modifiers. Anything else is reported by `finish`.
    for e in f.take_entries(|e| e.value.as_block().is_some()) {
        if let Some(b) = e.value.as_block() {
            t.unit_stats.push(parse_unit_stats(e.key.text, b, "technology.<unit>", ctx));
        }
    }
    for e in f.take_entries(|e| e.value.as_scalar().is_some_and(|s| !s.quoted && Fixed::parse(s.text).is_ok())) {
        if let Some(v) = ctx.fixed(e, "technology.<modifier>") {
            t.modifiers.push((e.key.text.to_string(), v));
        }
    }
    f.finish(ctx);
    t
}

fn parse_folder(entry: &Entry<'_>, constants: &Constants, ctx: &mut Ctx<'_, '_>) -> Option<FolderPosition> {
    const P: &str = "technology.folder";
    let b = ctx.block(entry, P)?;
    let mut f = Fields::new(b, P);
    let folder = f.take("name").and_then(|e| ctx.text(e, "technology.folder.name"));
    let mut pos = None;
    if let Some(e) = f.take("position")
        && let Some(pb) = ctx.block(e, "technology.folder.position")
    {
        let mut pf = Fields::new(pb, "technology.folder.position");
        let x = pf.take("x").and_then(|v| constants.fixed(ctx, v, "technology.folder.position.x"));
        let y = pf.take("y").and_then(|v| constants.fixed(ctx, v, "technology.folder.position.y"));
        pf.finish(ctx);
        match (x, y) {
            (Some(x), Some(y)) => pos = Some((x, y)),
            _ => ctx.issue(e.span, "technology.folder.position", IssueKind::Missing("x and y")),
        }
    }
    f.finish(ctx);
    match (folder, pos) {
        (Some(folder), Some((x, y))) => Some(FolderPosition { folder, x, y }),
        _ => {
            ctx.issue(entry.span, P, IssueKind::Missing("name and position"));
            None
        }
    }
}

fn parse_path(entry: &Entry<'_>, constants: &Constants, ctx: &mut Ctx<'_, '_>) -> Option<TechPath> {
    const P: &str = "technology.path";
    let b = ctx.block(entry, P)?;
    // `path = { }` occurs in the base game (three technologies) and says nothing.
    if b.items.is_empty() {
        return None;
    }
    let mut f = Fields::new(b, P);
    let leads_to = f.take("leads_to_tech").and_then(|e| ctx.text(e, "technology.path.leads_to_tech"));
    let research_cost_coeff =
        f.take("research_cost_coeff").and_then(|e| constants.fixed(ctx, e, "technology.path.research_cost_coeff"));
    let ignore_for_layout =
        f.take("ignore_for_layout").and_then(|e| ctx.bool(e, "technology.path.ignore_for_layout")).unwrap_or(false);
    f.finish(ctx);
    match leads_to {
        Some(leads_to) => Some(TechPath { leads_to, research_cost_coeff, ignore_for_layout }),
        None => {
            ctx.issue(entry.span, P, IssueKind::Missing("leads_to_tech"));
            None
        }
    }
}

/// `{ limit = { ... } name name ... }`: the names, and whether a trigger guards them.
fn parse_unlock(entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>, path: &str) -> Option<Unlock> {
    let b = ctx.block(entry, path)?;
    let mut f = Fields::new(b, path);
    let mut unlock = Unlock { conditional: b.entries().any(|e| e.key.text == "limit"), ..Default::default() };
    f.defer_matching(|k| k == "limit");
    for v in f.take_values() {
        match v.as_scalar() {
            Some(s) => unlock.names.push(s.unescaped().into_owned()),
            None => ctx.issue(v.span(), path, IssueKind::InvalidValue("a list of names")),
        }
    }
    f.finish(ctx);
    Some(unlock)
}

/// Reads `<sub-unit or category> = { ... }`. `path` is the schema path of the block.
pub(crate) fn parse_unit_stats(target: &str, block: &Block<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> UnitStats {
    let mut f = Fields::new(block, path);
    let mut u = UnitStats { target: target.to_string(), ..Default::default() };
    for e in f.take_all("battalion_mult") {
        u.battalion_mult.extend(parse_battalion_mult(e, ctx, path));
    }
    for e in f.take_entries(|e| e.value.as_block().is_some()) {
        let Some(b) = e.value.as_block() else { continue };
        let list = ctx.numbers(b, &format!("{path}.<terrain>"));
        if u.terrain.contains_key(e.key.text) {
            ctx.issue(e.span, format!("{path}.<terrain>"), IssueKind::Duplicate);
        } else {
            u.terrain.insert(e.key.text.to_string(), list);
        }
    }
    for e in f.take_matching(|_| true) {
        if let Some(v) = ctx.fixed(e, &format!("{path}.<stat>")) {
            u.stats.push((e.key.text.to_string(), v));
        }
    }
    f.finish(ctx);
    u
}
