//! Characters: `common/characters/*.txt`, block `characters`.
//!
//! Source: `common/characters/_documentation.md` (shipped with the game: the `characters` root, `can_be_captured`,
//! the scientist role) and a key inventory of the files (revision `8873ec77`, game 1.20.0.0) for the rest.
//!
//! A character has a name, portraits and *roles*: country leader (one per ideology), advisor (an idea in a character
//! slot), corps commander, field marshal, navy leader, scientist. A character may instead (or also) give
//! `instance = { allowed = { ... } ... }` blocks: alternative versions of the character, of which the script VM picks
//! one by its `allowed` trigger (for example a DLC check). Triggers and effects are deferred.

use std::collections::BTreeMap;

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::ideas::{Modifiers, parse_modifiers};
use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::{Date, NumberList};

/// Triggers, effects and AI weights of an advisor role: evaluated by the script VM.
pub const ADVISOR_SCRIPTS: [&str; 7] =
    ["allowed", "available", "visible", "do_effect", "on_add", "on_remove", "ai_will_do"];

/// Skill keys of military roles, in the order they are written.
pub const COMMANDER_SKILLS: [&str; 7] = [
    "skill",
    "attack_skill",
    "defense_skill",
    "planning_skill",
    "logistics_skill",
    "maneuvering_skill",
    "coordination_skill",
];

/// `portraits = { army = { large = X small = Y } civilian = { ... } navy = { ... } }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Portrait {
    /// `army`, `civilian` or `navy`.
    pub kind: String,
    pub large: Option<String>,
    pub small: Option<String>,
}

/// `country_leader = { ideology = X expire = "Y.M.D.H" id = N traits = { ... } }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CountryLeaderRole {
    /// A sub-ideology (`stalinism`, `despotism`).
    pub ideology: Option<String>,
    pub expire: Option<Date>,
    /// `id = N`, a legacy identifier (almost always `-1`).
    pub id: Option<i64>,
    pub traits: Vec<String>,
}

/// `advisor = { slot = X idea_token = Y ... }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AdvisorRole {
    /// A character slot of an idea category (`political_advisor`, `army_chief`).
    pub slot: Option<String>,
    /// The name of the idea the advisor becomes when hired.
    pub idea_token: Option<String>,
    /// `name = <localisation key>` for the advisor role.
    pub name: Option<String>,
    pub ledger: Option<String>,
    pub cost: Option<Fixed>,
    pub removal_cost: Option<Fixed>,
    pub can_be_fired: Option<bool>,
    pub always_show_on_actions_tooltip: Option<bool>,
    pub traits: Vec<String>,
    pub modifiers: Modifiers,
    /// `research_bonus = { <technology category> = N }`.
    pub research_bonus: NumberList,
    /// Which of [`ADVISOR_SCRIPTS`] the role has.
    pub scripts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommanderKind {
    CorpsCommander,
    FieldMarshal,
    NavyLeader,
}

impl CommanderKind {
    pub const ALL: [(CommanderKind, &'static str); 3] = [
        (CommanderKind::CorpsCommander, "corps_commander"),
        (CommanderKind::FieldMarshal, "field_marshal"),
        (CommanderKind::NavyLeader, "navy_leader"),
    ];

    pub fn key(self) -> &'static str {
        match self {
            CommanderKind::CorpsCommander => "corps_commander",
            CommanderKind::FieldMarshal => "field_marshal",
            CommanderKind::NavyLeader => "navy_leader",
        }
    }
}

/// `corps_commander`, `field_marshal` or `navy_leader = { skill = N attack_skill = N ... traits = { ... } }`.
#[derive(Debug, Clone, PartialEq)]
pub struct CommanderRole {
    pub kind: CommanderKind,
    /// Skill values by key (see [`COMMANDER_SKILLS`]), as written.
    pub skills: Vec<(String, i64)>,
    pub legacy_id: Option<i64>,
    pub traits: Vec<String>,
    /// The role has a `visible` trigger (deferred).
    pub conditional: bool,
}

impl CommanderRole {
    pub fn skill(&self, key: &str) -> Option<i64> {
        self.skills.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

/// `scientist = { desc = X traits = { ... } skills = { <specialization> = N } visible = { ... } }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScientistRole {
    pub desc: Option<String>,
    pub traits: Vec<String>,
    /// Skill level per special project specialisation; the documentation says the others start at level 1.
    pub skills: NumberList,
    pub conditional: bool,
}

/// What a character (or one instance of it) is made of.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CharacterBody {
    /// `name = <localisation key or literal name>`.
    pub name: Option<String>,
    pub gender: Option<String>,
    /// `can_be_captured = no`; the documentation gives `yes` as the default.
    pub can_be_captured: Option<bool>,
    pub portraits: Vec<Portrait>,
    pub country_leader: Vec<CountryLeaderRole>,
    pub advisors: Vec<AdvisorRole>,
    pub commanders: Vec<CommanderRole>,
    pub scientist: Option<ScientistRole>,
    /// Has an `allowed_civil_war` trigger (deferred): whether the character goes with a civil war side.
    pub has_allowed_civil_war: bool,
}

/// `instance = { allowed = { ... } <body> }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CharacterInstance {
    /// Has an `allowed` trigger (deferred; the script VM picks the instance).
    pub conditional: bool,
    pub body: CharacterBody,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CharacterDef {
    /// The token (`GER_adolf_hitler`) used by `recruit_character` and other effects.
    pub token: String,
    pub file: String,
    pub body: CharacterBody,
    pub instances: Vec<CharacterInstance>,
}

impl CharacterDef {
    /// The body and every instance body.
    pub fn bodies(&self) -> impl Iterator<Item = &CharacterBody> {
        std::iter::once(&self.body).chain(self.instances.iter().map(|i| &i.body))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Characters {
    /// In definition order.
    pub list: Vec<CharacterDef>,
    index: BTreeMap<String, usize>,
}

impl Characters {
    pub fn get(&self, token: &str) -> Option<&CharacterDef> {
        self.index.get(token).and_then(|&i| self.list.get(i))
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    fn insert(&mut self, def: CharacterDef) {
        self.index.insert(def.token.clone(), self.list.len());
        self.list.push(def);
    }
}

const DIR: &str = "common/characters";
const S: &str = "character";

pub fn load(vfs: &Vfs, report: &mut Report) -> Characters {
    let mut out = Characters::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

/// Reads one file. A second character with a token already defined is reported and the first kept.
pub fn parse_file(src: &Source, report: &mut Report, out: &mut Characters) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for list in top.take_all("characters") {
        let Some(block) = ctx.block(list, "characters") else { continue };
        let mut fields = Fields::new(block, "characters");
        for entry in fields.take_matching(|_| true) {
            let Some(body) = ctx.block(entry, S) else { continue };
            let def = parse_character(entry, body, &mut ctx);
            if out.get(&def.token).is_some() {
                ctx.issue(entry.span, S, IssueKind::Redefinition("first"));
            } else {
                out.insert(def);
            }
        }
        fields.finish(&mut ctx);
    }
    top.finish(&mut ctx);
}

fn parse_character(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> CharacterDef {
    let mut f = Fields::new(block, S);
    let mut def = CharacterDef { token: entry.key.text.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    for e in f.take_all("instance") {
        const P: &str = "character.instance";
        let Some(b) = ctx.block(e, P) else { continue };
        let mut inf = Fields::new(b, P);
        let conditional = b.entries().any(|e| e.key.text == "allowed");
        inf.defer_matching(|k| k == "allowed");
        let body = parse_body(&mut inf, P, ctx);
        inf.finish(ctx);
        def.instances.push(CharacterInstance { conditional, body });
    }
    def.body = parse_body(&mut f, S, ctx);
    f.finish(ctx);
    def
}

/// The keys shared by a character and its instances; `path` is the schema path of the enclosing block.
fn parse_body(f: &mut Fields<'_, '_>, path: &str, ctx: &mut Ctx<'_, '_>) -> CharacterBody {
    let mut b = CharacterBody::default();
    if let Some(e) = f.take("name") {
        b.name = ctx.text(e, &format!("{path}.name"));
    }
    if let Some(e) = f.take("gender") {
        b.gender = ctx.text(e, &format!("{path}.gender"));
    }
    if let Some(e) = f.take("can_be_captured") {
        b.can_be_captured = ctx.bool(e, &format!("{path}.can_be_captured"));
    }
    if let Some(e) = f.take("allowed_civil_war") {
        // The trigger runs in the script VM.
        b.has_allowed_civil_war = true;
        ctx.issue(e.span, format!("{path}.allowed_civil_war"), IssueKind::Deferred);
    }
    // A few characters split their portraits over two blocks (civilian in one, army in the other, PER.txt); all are
    // kept, in order.
    for e in f.take_all("portraits") {
        b.portraits.extend(parse_portraits(e, &format!("{path}.portraits"), ctx));
    }
    for e in f.take_all("country_leader") {
        b.country_leader.extend(parse_country_leader(e, &format!("{path}.country_leader"), ctx));
    }
    for e in f.take_all("advisor") {
        b.advisors.extend(parse_advisor(e, &format!("{path}.advisor"), ctx));
    }
    for (kind, key) in CommanderKind::ALL {
        for e in f.take_all(key) {
            b.commanders.extend(parse_commander(e, kind, &format!("{path}.{key}"), ctx));
        }
    }
    if let Some(e) = f.take("scientist") {
        b.scientist = parse_scientist(e, &format!("{path}.scientist"), ctx);
    }
    b
}

fn parse_portraits(entry: &Entry<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Vec<Portrait> {
    let Some(block) = ctx.block(entry, path) else { return Vec::new() };
    let mut out = Vec::new();
    let mut f = Fields::new(block, path);
    for e in f.take_matching(|_| true) {
        let p = format!("{path}.<kind>");
        let Some(b) = ctx.block(e, &p) else { continue };
        let mut pf = Fields::new(b, p.clone());
        let mut portrait = Portrait { kind: e.key.text.to_string(), ..Default::default() };
        if let Some(v) = pf.take("large") {
            portrait.large = ctx.text(v, &format!("{p}.large"));
        }
        if let Some(v) = pf.take("small") {
            portrait.small = ctx.text(v, &format!("{p}.small"));
        }
        pf.finish(ctx);
        out.push(portrait);
    }
    f.finish(ctx);
    out
}

fn parse_country_leader(entry: &Entry<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Option<CountryLeaderRole> {
    let block = ctx.block(entry, path)?;
    let mut f = Fields::new(block, path);
    let mut r = CountryLeaderRole::default();
    if let Some(e) = f.take("ideology") {
        r.ideology = ctx.text(e, &format!("{path}.ideology"));
    }
    if let Some(e) = f.take("expire")
        && let Some(text) = ctx.text(e, &format!("{path}.expire"))
    {
        r.expire = Date::parse(&text);
        if r.expire.is_none() {
            ctx.issue(e.value.span(), format!("{path}.expire"), IssueKind::InvalidValue("a date"));
        }
    }
    if let Some(e) = f.take("id") {
        r.id = ctx.int(e, &format!("{path}.id"));
    }
    for e in f.take_all("traits") {
        r.traits.extend(ctx.names(e, &format!("{path}.traits")));
    }
    f.finish(ctx);
    Some(r)
}

fn parse_advisor(entry: &Entry<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Option<AdvisorRole> {
    let block = ctx.block(entry, path)?;
    let mut f = Fields::new(block, path);
    let scripts =
        ADVISOR_SCRIPTS.iter().filter(|k| block.entries().any(|e| e.key.text == **k)).map(|k| k.to_string()).collect();
    let mut r = AdvisorRole { scripts, ..Default::default() };
    f.defer_matching(|k| ADVISOR_SCRIPTS.contains(&k));
    for (key, slot) in
        [("slot", &mut r.slot), ("idea_token", &mut r.idea_token), ("name", &mut r.name), ("ledger", &mut r.ledger)]
    {
        if let Some(e) = f.take(key) {
            *slot = ctx.text(e, &format!("{path}.{key}"));
        }
    }
    for (key, slot) in [("cost", &mut r.cost), ("removal_cost", &mut r.removal_cost)] {
        if let Some(e) = f.take(key) {
            *slot = ctx.fixed(e, &format!("{path}.{key}"));
        }
    }
    for (key, slot) in [
        ("can_be_fired", &mut r.can_be_fired),
        ("always_show_on_actions_tooltip", &mut r.always_show_on_actions_tooltip),
    ] {
        if let Some(e) = f.take(key) {
            *slot = ctx.bool(e, &format!("{path}.{key}"));
        }
    }
    for e in f.take_all("traits") {
        r.traits.extend(ctx.names(e, &format!("{path}.traits")));
    }
    if let Some(e) = f.take("modifier")
        && let Some(b) = ctx.block(e, &format!("{path}.modifier"))
    {
        r.modifiers = parse_modifiers(b, &format!("{path}.modifier"), ctx);
    }
    for e in f.take_all("research_bonus") {
        let p = format!("{path}.research_bonus");
        if let Some(b) = ctx.block(e, &p) {
            r.research_bonus.extend(ctx.numbers(b, &p));
        }
    }
    f.finish(ctx);
    Some(r)
}

fn parse_commander(entry: &Entry<'_>, kind: CommanderKind, path: &str, ctx: &mut Ctx<'_, '_>) -> Option<CommanderRole> {
    let block = ctx.block(entry, path)?;
    let mut f = Fields::new(block, path);
    let conditional = block.entries().any(|e| e.key.text == "visible");
    f.defer_matching(|k| k == "visible");
    let mut skills = Vec::new();
    for key in COMMANDER_SKILLS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.int(e, &format!("{path}.{key}"))
        {
            skills.push((key.to_string(), v));
        }
    }
    let legacy_id = f.take("legacy_id").and_then(|e| ctx.int(e, &format!("{path}.legacy_id")));
    let mut traits = Vec::new();
    for e in f.take_all("traits") {
        traits.extend(ctx.names(e, &format!("{path}.traits")));
    }
    f.finish(ctx);
    Some(CommanderRole { kind, skills, legacy_id, traits, conditional })
}

fn parse_scientist(entry: &Entry<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Option<ScientistRole> {
    let block = ctx.block(entry, path)?;
    let mut f = Fields::new(block, path);
    let mut r = ScientistRole { conditional: block.entries().any(|e| e.key.text == "visible"), ..Default::default() };
    f.defer_matching(|k| k == "visible");
    if let Some(e) = f.take("desc") {
        r.desc = ctx.text(e, &format!("{path}.desc"));
    }
    for e in f.take_all("traits") {
        r.traits.extend(ctx.names(e, &format!("{path}.traits")));
    }
    if let Some(e) = f.take("skills")
        && let Some(b) = ctx.block(e, &format!("{path}.skills"))
    {
        r.skills = ctx.numbers(b, &format!("{path}.skills"));
    }
    f.finish(ctx);
    Some(r)
}
