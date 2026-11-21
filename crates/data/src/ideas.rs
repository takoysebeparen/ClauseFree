//! Ideas (national spirits, laws, designers, advisor ideas): `common/ideas/*.txt` (block `ideas`) and the categories
//! and slots they belong to, `common/idea_tags/*.txt` (`idea_categories`, `slot_ledgers`).
//!
//! Source: the files and a key inventory of them (revision `8873ec77`, game 1.20.0.0), including the comments at the
//! top of `common/idea_tags/00_idea.txt` (ledger precedence: "idea > idea slot > idea category"). An `ideas` block
//! groups ideas under a *group* key, which is an idea category (`country`, `hidden_ideas`) or a slot of one
//! (`mobilization_laws`, `tank_manufacturer`); the group may say `law = yes`, `designer = yes` or
//! `use_list_view = yes` for all of its ideas.
//!
//! Values are stored as written; what they do is decided by the systems that apply modifiers (see `docs/CANON.md`,
//! "Ideas"). Triggers and effects (`allowed`, `available`, `on_add`, ...) are deferred to the script VM.

use std::collections::BTreeMap;

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::NumberList;

/// Triggers, effects and AI weights of an idea: understood, evaluated by the script VM.
pub const IDEA_SCRIPTS: [&str; 10] = [
    "allowed",
    "allowed_civil_war",
    "allowed_to_remove",
    "available",
    "visible",
    "cancel",
    "do_effect",
    "on_add",
    "on_remove",
    "ai_will_do",
];

/// A modifier block (`modifier = { ... }`): country modifiers as written, plus the display-only parts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Modifiers {
    pub values: NumberList,
    /// `custom_modifier_tooltip = <localisation key>`: extra tooltip text.
    pub custom_tooltips: Vec<String>,
    /// `hidden_modifier = { ... }`: modifiers that apply but are not shown.
    pub hidden: NumberList,
    /// `<terrain> = { movement = 0.1 }`: modifiers that apply in one terrain. Only read where a loader allows them
    /// (leader traits, see [`parse_modifiers_with_terrain`]); the terrain names are checked by the database.
    pub terrain: Vec<(String, NumberList)>,
}

impl Modifiers {
    pub fn get(&self, name: &str) -> Option<Fixed> {
        self.values.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.custom_tooltips.is_empty() && self.hidden.is_empty() && self.terrain.is_empty()
    }
}

/// Reads a modifier block. `path` is the schema path of the block.
pub fn parse_modifiers(block: &Block<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Modifiers {
    read_modifiers(block, path, false, ctx)
}

/// Reads a modifier block that may also hold `<terrain> = { ... }` blocks (any other block-valued key is taken as a
/// terrain and checked against the terrain categories by the database).
pub fn parse_modifiers_with_terrain(block: &Block<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Modifiers {
    read_modifiers(block, path, true, ctx)
}

fn read_modifiers(block: &Block<'_>, path: &str, terrain: bool, ctx: &mut Ctx<'_, '_>) -> Modifiers {
    let mut f = Fields::new(block, path);
    let mut m = Modifiers::default();
    for e in f.take_all("custom_modifier_tooltip") {
        m.custom_tooltips.extend(ctx.text(e, &format!("{path}.custom_modifier_tooltip")));
    }
    for e in f.take_all("hidden_modifier") {
        let p = format!("{path}.hidden_modifier");
        if let Some(b) = ctx.block(e, &p) {
            m.hidden.extend(ctx.numbers(b, &p));
        }
    }
    for e in f.take_entries(|e| e.value.as_scalar().is_some()) {
        if let Some(v) = ctx.fixed(e, &format!("{path}.<modifier>")) {
            m.values.push((e.key.text.to_string(), v));
        }
    }
    if terrain {
        for e in f.take_entries(|e| e.value.as_block().is_some()) {
            let p = format!("{path}.<terrain>");
            if let Some(b) = ctx.block(e, &p) {
                m.terrain.push((e.key.text.to_string(), ctx.numbers(b, &p)));
            }
        }
    }
    f.finish(ctx);
    m
}

/// `targeted_modifier = { tag = X <modifier> = N }`: modifiers that apply towards one country.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TargetedModifier {
    /// A tag or a scope reference (`ROOT`, `event_target:...`, `JAP.JAP_kantai_kessen_target_1`), resolved by the
    /// script VM.
    pub target: String,
    pub modifiers: Modifiers,
}

/// `equipment_bonus = { <equipment or equipment type> = { <stat> = N instant = yes } }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EquipmentBonus {
    /// An equipment (`infantry_equipment`) or an equipment type (`armor`); checked by the database.
    pub target: String,
    pub stats: NumberList,
    /// `instant = yes`: the bonus applies to existing equipment too, not only to new variants (wiki; unmeasured).
    pub instant: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct IdeaDef {
    pub name: String,
    /// The group key the idea is defined under: an idea category or a slot.
    pub group: String,
    pub file: String,
    /// Set by `law = yes` on the group.
    pub law: bool,
    /// Set by `designer = yes` on the group.
    pub designer: bool,
    /// Set by `use_list_view = yes` on the group.
    pub use_list_view: bool,
    /// `name = <localisation key>`: a name key other than the idea's own.
    pub name_key: Option<String>,
    pub picture: Option<String>,
    pub ledger: Option<String>,
    pub cost: Option<Fixed>,
    pub removal_cost: Option<Fixed>,
    /// `level = N`: order of a law within its slot.
    pub level: Option<i64>,
    /// `default = yes`: the law a country has when none is set.
    pub default: bool,
    pub cancel_if_invalid: Option<bool>,
    pub traits: Vec<String>,
    pub modifiers: Modifiers,
    pub targeted_modifiers: Vec<TargetedModifier>,
    /// `research_bonus = { <technology category> = N }`.
    pub research_bonus: NumberList,
    pub equipment_bonus: Vec<EquipmentBonus>,
    /// `rule = { <game rule> = yes/no }`.
    pub rules: Vec<(String, bool)>,
    /// `rule = { desc = <localisation key> }`.
    pub rule_desc: Option<String>,
    /// Which of [`IDEA_SCRIPTS`] the idea has (the scripts themselves are deferred).
    pub scripts: Vec<String>,
}

impl IdeaDef {
    pub fn has_script(&self, key: &str) -> bool {
        self.scripts.iter().any(|s| s == key)
    }
}

/// `idea_categories = { <name> = { ... } }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IdeaCategory {
    pub name: String,
    /// `slot = X`, in order: idea slots of the category.
    pub slots: Vec<String>,
    /// `character_slot = X`, in order and with repeats (a repeat is a second slot of the same kind).
    pub character_slots: Vec<String>,
    /// `type = X` (`national_spirit`, `army_spirit`, ...).
    pub kind: Option<String>,
    pub hidden: bool,
    pub politics_tab: Option<bool>,
    pub ledger: Option<String>,
    pub cost: Option<Fixed>,
    pub removal_cost: Option<Fixed>,
}

#[derive(Debug, Clone, Default)]
pub struct Ideas {
    /// In definition order.
    pub list: Vec<IdeaDef>,
    index: BTreeMap<String, usize>,
    /// In definition order.
    pub categories: Vec<IdeaCategory>,
    /// `slot_ledgers = { <slot> = <ledger> }`.
    pub slot_ledgers: Vec<(String, String)>,
}

impl Ideas {
    pub fn get(&self, name: &str) -> Option<&IdeaDef> {
        self.index.get(name).and_then(|&i| self.list.get(i))
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn category(&self, name: &str) -> Option<&IdeaCategory> {
        self.categories.iter().find(|c| c.name == name)
    }

    /// The category that declares this idea slot (`slot = X`).
    pub fn category_of_slot(&self, slot: &str) -> Option<&IdeaCategory> {
        self.categories.iter().find(|c| c.slots.iter().any(|s| s == slot))
    }

    /// Whether some category declares this character slot (`character_slot = X`).
    pub fn has_character_slot(&self, slot: &str) -> bool {
        self.categories.iter().any(|c| c.character_slots.iter().any(|s| s == slot))
    }

    fn insert(&mut self, def: IdeaDef) {
        self.index.insert(def.name.clone(), self.list.len());
        self.list.push(def);
    }
}

const DIR: &str = "common/ideas";
const TAGS_DIR: &str = "common/idea_tags";
const S: &str = "idea";

pub fn load(vfs: &Vfs, report: &mut Report) -> Ideas {
    let mut out = Ideas::default();
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

/// Reads `idea_categories` and `slot_ledgers`. A repeated category or slot ledger is reported and the first kept.
pub fn parse_tags(src: &Source, report: &mut Report, out: &mut Ideas) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for e in top.take_all("idea_categories") {
        const P: &str = "idea_categories";
        let Some(b) = ctx.block(e, P) else { continue };
        let mut cf = Fields::new(b, P);
        for entry in cf.take_matching(|_| true) {
            let Some(body) = ctx.block(entry, "idea_category") else { continue };
            let c = parse_category(entry.key.text, body, &mut ctx);
            if out.category(&c.name).is_some() {
                ctx.issue(entry.span, "idea_category", IssueKind::Redefinition("first"));
            } else {
                out.categories.push(c);
            }
        }
        cf.finish(&mut ctx);
    }
    for e in top.take_all("slot_ledgers") {
        const P: &str = "slot_ledgers";
        let Some(b) = ctx.block(e, P) else { continue };
        let mut lf = Fields::new(b, P);
        for entry in lf.take_matching(|_| true) {
            let Some(ledger) = ctx.text(entry, "slot_ledgers.<slot>") else { continue };
            if out.slot_ledgers.iter().any(|(s, _)| s == entry.key.text) {
                ctx.issue(entry.span, "slot_ledgers.<slot>", IssueKind::Redefinition("first"));
            } else {
                out.slot_ledgers.push((entry.key.text.to_string(), ledger));
            }
        }
        lf.finish(&mut ctx);
    }
    top.finish(&mut ctx);
}

fn parse_category(name: &str, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> IdeaCategory {
    const P: &str = "idea_category";
    let mut f = Fields::new(block, P);
    let mut c = IdeaCategory { name: name.to_string(), ..Default::default() };
    for e in f.take_all("slot") {
        c.slots.extend(ctx.text(e, "idea_category.slot"));
    }
    for e in f.take_all("character_slot") {
        c.character_slots.extend(ctx.text(e, "idea_category.character_slot"));
    }
    if let Some(e) = f.take("type") {
        c.kind = ctx.text(e, "idea_category.type");
    }
    if let Some(e) = f.take("hidden") {
        c.hidden = ctx.bool(e, "idea_category.hidden").unwrap_or(false);
    }
    if let Some(e) = f.take("politics_tab") {
        c.politics_tab = ctx.bool(e, "idea_category.politics_tab");
    }
    if let Some(e) = f.take("ledger") {
        c.ledger = ctx.text(e, "idea_category.ledger");
    }
    if let Some(e) = f.take("cost") {
        c.cost = ctx.fixed(e, "idea_category.cost");
    }
    if let Some(e) = f.take("removal_cost") {
        c.removal_cost = ctx.fixed(e, "idea_category.removal_cost");
    }
    f.finish(ctx);
    c
}

/// Reads one `common/ideas` file. A second idea with a name already defined is reported and the first kept.
pub fn parse_file(src: &Source, report: &mut Report, out: &mut Ideas) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for list in top.take_all("ideas") {
        let Some(block) = ctx.block(list, "ideas") else { continue };
        let mut groups = Fields::new(block, "ideas");
        for group in groups.take_matching(|_| true) {
            let Some(gb) = ctx.block(group, "ideas.<group>") else { continue };
            let mut gf = Fields::new(gb, "ideas.<group>");
            let flag = |gf: &mut Fields<'_, '_>, ctx: &mut Ctx<'_, '_>, key: &str| {
                // Written anywhere in the group, also after its ideas and sometimes twice (ethiopia.txt); it holds for
                // the whole group.
                let mut on = false;
                for e in gf.take_all(key) {
                    on |= ctx.bool(e, &format!("ideas.<group>.{key}")).unwrap_or(false);
                }
                on
            };
            let law = flag(&mut gf, &mut ctx, "law");
            let designer = flag(&mut gf, &mut ctx, "designer");
            let use_list_view = flag(&mut gf, &mut ctx, "use_list_view");
            for entry in gf.take_matching(|_| true) {
                let Some(body) = ctx.block(entry, S) else { continue };
                let mut def = parse_idea(entry, body, &mut ctx);
                def.group = group.key.text.to_string();
                def.law = law;
                def.designer = designer;
                def.use_list_view = use_list_view;
                if out.get(&def.name).is_some() {
                    ctx.issue(entry.span, S, IssueKind::Redefinition("first"));
                } else {
                    out.insert(def);
                }
            }
            gf.finish(&mut ctx);
        }
        groups.finish(&mut ctx);
    }
    top.finish(&mut ctx);
}

fn parse_idea(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> IdeaDef {
    let mut f = Fields::new(block, S);
    let mut d = IdeaDef { name: entry.key.text.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    d.scripts =
        IDEA_SCRIPTS.iter().filter(|k| block.entries().any(|e| e.key.text == **k)).map(|k| k.to_string()).collect();
    f.defer_matching(|k| IDEA_SCRIPTS.contains(&k));
    for (key, slot) in [("name", &mut d.name_key), ("picture", &mut d.picture), ("ledger", &mut d.ledger)] {
        if let Some(e) = f.take(key) {
            *slot = ctx.text(e, &format!("{S}.{key}"));
        }
    }
    for (key, slot) in [("cost", &mut d.cost), ("removal_cost", &mut d.removal_cost)] {
        if let Some(e) = f.take(key) {
            *slot = ctx.fixed(e, &format!("{S}.{key}"));
        }
    }
    if let Some(e) = f.take("level") {
        d.level = ctx.int(e, "idea.level");
    }
    if let Some(e) = f.take("default") {
        d.default = ctx.bool(e, "idea.default").unwrap_or(false);
    }
    if let Some(e) = f.take("cancel_if_invalid") {
        d.cancel_if_invalid = ctx.bool(e, "idea.cancel_if_invalid");
    }
    for e in f.take_all("traits") {
        d.traits.extend(ctx.names(e, "idea.traits"));
    }
    if let Some(e) = f.take("modifier")
        && let Some(b) = ctx.block(e, "idea.modifier")
    {
        d.modifiers = parse_modifiers(b, "idea.modifier", ctx);
    }
    for e in f.take_all("targeted_modifier") {
        d.targeted_modifiers.extend(parse_targeted(e, "idea.targeted_modifier", ctx));
    }
    // Repeated bonus blocks occur (16 ideas write one `equipment_bonus` per equipment); all are kept, in order.
    for e in f.take_all("research_bonus") {
        if let Some(b) = ctx.block(e, "idea.research_bonus") {
            d.research_bonus.extend(ctx.numbers(b, "idea.research_bonus"));
        }
    }
    for e in f.take_all("equipment_bonus") {
        if let Some(b) = ctx.block(e, "idea.equipment_bonus") {
            d.equipment_bonus.extend(parse_equipment_bonus(b, "idea.equipment_bonus", ctx));
        }
    }
    if let Some(e) = f.take("rule")
        && let Some(b) = ctx.block(e, "idea.rule")
    {
        let mut rf = Fields::new(b, "idea.rule");
        if let Some(desc) = rf.take("desc") {
            d.rule_desc = ctx.text(desc, "idea.rule.desc");
        }
        for r in rf.take_matching(|_| true) {
            if let Some(v) = ctx.bool(r, "idea.rule.<rule>") {
                d.rules.push((r.key.text.to_string(), v));
            }
        }
        rf.finish(ctx);
    }
    f.finish(ctx);
    d
}

/// Reads `targeted_modifier = { tag = X <modifier> = N }`. `path` is the schema path of the block.
pub(crate) fn parse_targeted(entry: &Entry<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Option<TargetedModifier> {
    let b = ctx.block(entry, path)?;
    let mut f = Fields::new(b, path);
    let target = f.take("tag").and_then(|e| ctx.text(e, &format!("{path}.tag")));
    // The rest has the shape of a modifier block; read it with the `tag` entry already consumed.
    let mut m = Modifiers::default();
    for e in f.take_all("custom_modifier_tooltip") {
        m.custom_tooltips.extend(ctx.text(e, &format!("{path}.custom_modifier_tooltip")));
    }
    for e in f.take_entries(|e| e.value.as_scalar().is_some()) {
        if let Some(v) = ctx.fixed(e, &format!("{path}.<modifier>")) {
            m.values.push((e.key.text.to_string(), v));
        }
    }
    f.finish(ctx);
    match target {
        Some(target) => Some(TargetedModifier { target, modifiers: m }),
        None => {
            ctx.issue(entry.span, path, IssueKind::Missing("tag"));
            None
        }
    }
}

/// Reads the inside of `equipment_bonus = { <equipment> = { <stat> = N instant = yes } }`. `path` is the schema path
/// of the block.
pub(crate) fn parse_equipment_bonus(block: &Block<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Vec<EquipmentBonus> {
    let p = format!("{path}.<equipment>");
    let mut out = Vec::new();
    let mut f = Fields::new(block, path);
    for e in f.take_matching(|_| true) {
        let Some(b) = ctx.block(e, &p) else { continue };
        let mut bf = Fields::new(b, p.clone());
        // `instant = yes` is often written after every stat (`build_cost_ic = -0.15 instant = yes`); it holds for the
        // whole block.
        let mut instant = false;
        for v in bf.take_all("instant") {
            instant |= ctx.bool(v, &format!("{p}.instant")).unwrap_or(false);
        }
        let mut stats = Vec::new();
        for s in bf.take_matching(|_| true) {
            if let Some(v) = ctx.fixed(s, &format!("{p}.<stat>")) {
                stats.push((s.key.text.to_string(), v));
            }
        }
        bf.finish(ctx);
        out.push(EquipmentBonus { target: e.key.text.to_string(), stats, instant });
    }
    f.finish(ctx);
    out
}
