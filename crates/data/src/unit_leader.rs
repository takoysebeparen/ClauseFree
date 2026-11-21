//! Unit leader skills and traits: `common/unit_leader/*.txt`.
//!
//! Source: the schema comment at the top of `common/unit_leader/00_traits.txt` (shipped with the game). A file holds
//! any of these top-level blocks:
//! - `leader_traits = { <trait> = { ... } }`;
//! - `leader_skills`, `leader_attack_skills`, `leader_defense_skills`, `leader_planning_skills`,
//!   `leader_logistics_skills`, `leader_maneuvering_skills`, `leader_coordination_skills`:
//!   `<level> = { cost type modifier building_module_modifier }`, what one level of that skill gives.
//!
//! Triggers, effects and weights are recorded by key and deferred to the script VM. Values are stored as written.

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::ideas::{Modifiers, parse_modifiers, parse_modifiers_with_terrain};
use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::NumberList;

/// Top-level skill tables and the skill each one describes (`leader_skills` is the overall level).
pub const SKILL_TABLES: [(&str, &str); 7] = [
    ("leader_skills", "skill"),
    ("leader_attack_skills", "attack"),
    ("leader_defense_skills", "defense"),
    ("leader_planning_skills", "planning"),
    ("leader_logistics_skills", "logistics"),
    ("leader_maneuvering_skills", "maneuvering"),
    ("leader_coordination_skills", "coordination"),
];

/// Skill numbers of a trait (`attack_skill = 1`, `attack_skill_factor = 0.5`, ...).
pub const TRAIT_SKILLS: [&str; 12] = [
    "attack_skill",
    "defense_skill",
    "logistics_skill",
    "planning_skill",
    "maneuvering_skill",
    "coordination_skill",
    "attack_skill_factor",
    "defense_skill_factor",
    "logistics_skill_factor",
    "planning_skill_factor",
    "maneuvering_skill_factor",
    "coordination_skill_factor",
];

/// Modifier blocks of a trait, by key.
pub const TRAIT_MODIFIER_BLOCKS: [&str; 4] =
    ["modifier", "non_shared_modifier", "corps_commander_modifier", "field_marshal_modifier"];

/// Script blocks of a trait: triggers, effects and weights.
pub const TRAIT_SCRIPTS: [&str; 11] = [
    "allowed",
    "prerequisites",
    "gain_xp",
    "gain_xp_leader",
    "unit_trigger",
    "country_trigger",
    "on_add",
    "on_remove",
    "daily_effect",
    "ai_will_do",
    "new_commander_weight",
];

/// Localisation keys of a trait's tooltips.
pub const TRAIT_TOOLTIPS: [&str; 4] = [
    "override_effect_tooltip",
    "custom_effect_tooltip",
    "custom_prerequisite_tooltip",
    "custom_gain_xp_trigger_tooltip",
];

/// One level of a skill table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkillLevel {
    /// The skill (see [`SKILL_TABLES`]).
    pub skill: String,
    pub level: u32,
    /// `type = navy | corps_commander | field_marshal | operative`.
    pub leader_type: Option<String>,
    pub cost: Option<Fixed>,
    pub modifiers: Modifiers,
    /// `building_module_modifier = { ... }`.
    pub building_module_modifiers: Modifiers,
    pub file: String,
}

/// `parent = { traits = { ... } num_parents_needed = N }`, or its shorthands `any_parent` (N = 1) and `all_parents`
/// (N = the number of traits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraitParents {
    pub traits: Vec<String>,
    pub needed: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LeaderTrait {
    pub name: String,
    pub file: String,
    /// `type = all | land | navy | corps_commander | field_marshal | operative`, or a list of them.
    pub types: Vec<String>,
    /// `trait_type = basic_trait | status_trait | personality_trait | assignable_trait | ...`.
    pub trait_type: Option<String>,
    /// Keys of [`TRAIT_SKILLS`] present.
    pub skills: NumberList,
    /// Keys of [`TRAIT_MODIFIER_BLOCKS`] present, with their contents.
    pub modifier_blocks: Vec<(String, Modifiers)>,
    /// `sub_unit_modifiers = { <sub-unit> = { <modifiers> } }`.
    pub sub_unit_modifiers: Vec<(String, Modifiers)>,
    /// `trait_xp_factor = { <trait> = factor }`.
    pub trait_xp_factor: NumberList,
    pub parents: Vec<TraitParents>,
    pub mutually_exclusive: Vec<String>,
    pub gui_row: Option<i64>,
    pub gui_column: Option<i64>,
    pub cost: Option<Fixed>,
    pub gain_xp_on_spotting: Option<Fixed>,
    pub show_in_combat: Option<bool>,
    pub leader_default_proximity_offset: Option<Fixed>,
    /// `unit_type = { type = cavalry ... }`: sub-unit types the trait applies to.
    pub unit_types: Vec<String>,
    /// `allowed_ship_equipments = { ... }`: equipment (hull) names.
    pub allowed_ship_equipments: Vec<String>,
    /// `enable_ability = <ability>`.
    pub abilities: Vec<String>,
    /// Advisor that the trait turns the leader into: `slot`, then the three advisor traits by level.
    pub advisor_slot: Option<String>,
    pub advisor_traits: Vec<(String, String)>,
    /// Keys of [`TRAIT_TOOLTIPS`] present, with their localisation keys.
    pub tooltips: Vec<(String, String)>,
    /// Keys of [`TRAIT_SCRIPTS`] present.
    pub scripts: Vec<String>,
}

impl LeaderTrait {
    pub fn modifiers(&self, block: &str) -> Option<&Modifiers> {
        self.modifier_blocks.iter().find(|(k, _)| k == block).map(|(_, m)| m)
    }

    pub fn skill(&self, key: &str) -> Option<Fixed> {
        self.skills.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

#[derive(Debug, Clone, Default)]
pub struct UnitLeaders {
    /// In definition order.
    pub traits: Vec<LeaderTrait>,
    /// In definition order.
    pub skills: Vec<SkillLevel>,
}

impl UnitLeaders {
    pub fn get_trait(&self, name: &str) -> Option<&LeaderTrait> {
        self.traits.iter().find(|t| t.name == name)
    }

    pub fn skill_level(&self, skill: &str, leader_type: &str, level: u32) -> Option<&SkillLevel> {
        self.skills
            .iter()
            .find(|s| s.skill == skill && s.level == level && s.leader_type.as_deref() == Some(leader_type))
    }
}

const DIR: &str = "common/unit_leader";
const T: &str = "leader_trait";

pub fn load(vfs: &Vfs, report: &mut Report) -> UnitLeaders {
    let mut out = UnitLeaders::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

/// Reads one file. A second trait with a name already defined, or a second level of the same skill and leader type,
/// is reported and the first kept.
pub fn parse_file(src: &Source, report: &mut Report, out: &mut UnitLeaders) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for entry in top.take_all("leader_traits") {
        let Some(block) = ctx.block(entry, "leader_traits") else { continue };
        let mut f = Fields::new(block, "leader_traits");
        for e in f.take_matching(|_| true) {
            let Some(b) = ctx.block(e, T) else { continue };
            let t = parse_trait(e, b, &mut ctx);
            if out.get_trait(&t.name).is_some() {
                ctx.issue(e.span, T, IssueKind::Redefinition("first"));
            } else {
                out.traits.push(t);
            }
        }
        f.finish(&mut ctx);
    }
    for (table, skill) in SKILL_TABLES {
        for entry in top.take_all(table) {
            let Some(block) = ctx.block(entry, table) else { continue };
            let mut f = Fields::new(block, table);
            for e in f.take_matching(|_| true) {
                let path = format!("{table}.<level>");
                let Ok(level) = e.key.text.parse::<u32>() else {
                    ctx.issue(e.span, &path, IssueKind::InvalidValue("a level number"));
                    continue;
                };
                let Some(b) = ctx.block(e, &path) else { continue };
                let s = parse_skill_level(skill, level, b, &path, &mut ctx);
                let lt = s.leader_type.clone().unwrap_or_default();
                if out.skill_level(skill, &lt, level).is_some() {
                    ctx.issue(e.span, &path, IssueKind::Redefinition("first"));
                } else {
                    out.skills.push(s);
                }
            }
            f.finish(&mut ctx);
        }
    }
    top.finish(&mut ctx);
}

fn parse_skill_level(skill: &str, level: u32, block: &Block<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> SkillLevel {
    let mut f = Fields::new(block, path);
    let mut s = SkillLevel { skill: skill.to_string(), level, file: ctx.src.path.clone(), ..Default::default() };
    if let Some(e) = f.take("type") {
        s.leader_type = ctx.text(e, &format!("{path}.type"));
    }
    if let Some(e) = f.take("cost") {
        s.cost = ctx.fixed(e, &format!("{path}.cost"));
    }
    if let Some(e) = f.take("modifier")
        && let Some(b) = ctx.block(e, &format!("{path}.modifier"))
    {
        s.modifiers = parse_modifiers(b, &format!("{path}.modifier"), ctx);
    }
    if let Some(e) = f.take("building_module_modifier")
        && let Some(b) = ctx.block(e, &format!("{path}.building_module_modifier"))
    {
        s.building_module_modifiers = parse_modifiers(b, &format!("{path}.building_module_modifier"), ctx);
    }
    f.finish(ctx);
    if s.leader_type.is_none() {
        ctx.issue(block.span, path, IssueKind::Missing("type"));
    }
    s
}

fn parse_trait(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> LeaderTrait {
    let mut f = Fields::new(block, T);
    let mut t = LeaderTrait { name: entry.key.text.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    let p = |key: &str| format!("{T}.{key}");
    if let Some(e) = f.take("type") {
        t.types = ctx.names(e, &p("type"));
    }
    if let Some(e) = f.take("trait_type") {
        t.trait_type = ctx.text(e, &p("trait_type"));
    }
    for key in TRAIT_SKILLS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.fixed(e, &p(key))
        {
            t.skills.push((key.to_string(), v));
        }
    }
    for key in TRAIT_MODIFIER_BLOCKS {
        if let Some(e) = f.take(key)
            && let Some(b) = ctx.block(e, &p(key))
        {
            t.modifier_blocks.push((key.to_string(), parse_modifiers_with_terrain(b, &p(key), ctx)));
        }
    }
    if let Some(e) = f.take("sub_unit_modifiers")
        && let Some(b) = ctx.block(e, &p("sub_unit_modifiers"))
    {
        let path = p("sub_unit_modifiers");
        let mut s = Fields::new(b, path.clone());
        for u in s.take_matching(|_| true) {
            let inner = format!("{path}.<sub_unit>");
            if let Some(ub) = ctx.block(u, &inner) {
                t.sub_unit_modifiers.push((u.key.text.to_string(), parse_modifiers(ub, &inner, ctx)));
            }
        }
        s.finish(ctx);
    }
    if let Some(e) = f.take("trait_xp_factor")
        && let Some(b) = ctx.block(e, &p("trait_xp_factor"))
    {
        t.trait_xp_factor = ctx.numbers(b, &p("trait_xp_factor"));
    }
    for e in f.take_all("parent") {
        t.parents.extend(parent_block(e, ctx));
    }
    for e in f.take_all("any_parent") {
        let traits = ctx.names(e, &p("any_parent"));
        t.parents.push(TraitParents { traits, needed: 1 });
    }
    for e in f.take_all("all_parents") {
        let traits = ctx.names(e, &p("all_parents"));
        let needed = traits.len();
        t.parents.push(TraitParents { traits, needed });
    }
    for e in f.take_all("mutually_exclusive") {
        t.mutually_exclusive.extend(ctx.names(e, &p("mutually_exclusive")));
    }
    if let Some(e) = f.take("gui_row") {
        t.gui_row = ctx.int(e, &p("gui_row"));
    }
    if let Some(e) = f.take("gui_column") {
        t.gui_column = ctx.int(e, &p("gui_column"));
    }
    if let Some(e) = f.take("cost") {
        t.cost = ctx.fixed(e, &p("cost"));
    }
    if let Some(e) = f.take("gain_xp_on_spotting") {
        t.gain_xp_on_spotting = ctx.fixed(e, &p("gain_xp_on_spotting"));
    }
    if let Some(e) = f.take("show_in_combat") {
        t.show_in_combat = ctx.bool(e, &p("show_in_combat"));
    }
    if let Some(e) = f.take("leader_default_proximity_offset") {
        t.leader_default_proximity_offset = ctx.fixed(e, &p("leader_default_proximity_offset"));
    }
    for e in f.take_all("unit_type") {
        let Some(b) = ctx.block(e, &p("unit_type")) else { continue };
        let mut u = Fields::new(b, p("unit_type"));
        for ty in u.take_all("type") {
            t.unit_types.extend(ctx.text(ty, &p("unit_type.type")));
        }
        u.finish(ctx);
    }
    for e in f.take_all("allowed_ship_equipments") {
        t.allowed_ship_equipments.extend(ctx.names(e, &p("allowed_ship_equipments")));
    }
    for e in f.take_all("enable_ability") {
        t.abilities.extend(ctx.names(e, &p("enable_ability")));
    }
    if let Some(e) = f.take("slot") {
        t.advisor_slot = ctx.text(e, &p("slot"));
    }
    for key in ["specialist_advisor_trait", "expert_advisor_trait", "genius_advisor_trait"] {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.text(e, &p(key))
        {
            t.advisor_traits.push((key.to_string(), v));
        }
    }
    for key in TRAIT_TOOLTIPS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.text(e, &p(key))
        {
            t.tooltips.push((key.to_string(), v));
        }
    }
    for key in TRAIT_SCRIPTS {
        if f.take(key).is_some() {
            t.scripts.push(key.to_string());
        }
    }
    f.finish(ctx);
    for key in &t.scripts {
        ctx.issue(entry.span, format!("{T}.{key}"), IssueKind::Deferred);
    }
    t
}

fn parent_block(entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>) -> Option<TraitParents> {
    const P: &str = "leader_trait.parent";
    let block = ctx.block(entry, P)?;
    let mut f = Fields::new(block, P);
    let traits = f.take("traits").map(|e| ctx.names(e, "leader_trait.parent.traits")).unwrap_or_default();
    let needed = match f.take("num_parents_needed") {
        Some(e) => ctx.u32(e, "leader_trait.parent.num_parents_needed").map(|n| n as usize),
        // Default per the schema comment in 00_traits.txt ("defaulted to 1").
        None => Some(1),
    };
    f.finish(ctx);
    needed.map(|needed| TraitParents { traits, needed })
}
