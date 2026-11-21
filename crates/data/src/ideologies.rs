//! Ideologies: `common/ideologies/*.txt`.
//!
//! The files hold `ideologies = { <ideology> = { ... } }`. An ideology has sub-types (leader ideologies such as
//! `stalinism`), faction names, a map colour, rules (what its countries may do), country modifiers, faction
//! modifiers, world tension factors and AI switches. Values are stored as written; their effects are decided by
//! the systems that use them.

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::ideas::{Modifiers, parse_modifiers};
use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::Color;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IdeologyType {
    pub name: String,
    /// `can_be_randomly_selected = no`: not given to generated country leaders.
    pub can_be_randomly_selected: Option<bool>,
}

/// Numbers of an ideology, by key.
pub const IDEOLOGY_NUMBERS: [&str; 4] = [
    "war_impact_on_world_tension",
    "faction_impact_on_world_tension",
    "ai_ideology_wanted_units_factor",
    "ai_give_core_state_control_threshold",
];

/// Yes/no switches of an ideology, by key. `ai_<x> = yes` selects the AI behaviour.
pub const IDEOLOGY_SWITCHES: [&str; 6] =
    ["can_host_government_in_exile", "can_collaborate", "ai_democratic", "ai_communist", "ai_fascist", "ai_neutral"];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ideology {
    pub name: String,
    pub file: String,
    pub types: Vec<IdeologyType>,
    /// `dynamic_faction_names = { "KEY" ... }`: localisation keys.
    pub faction_names: Vec<String>,
    pub color: Option<Color>,
    /// `rules = { can_puppet = yes ... }`, in file order.
    pub rules: Vec<(String, bool)>,
    pub modifiers: Modifiers,
    pub faction_modifiers: Modifiers,
    /// Keys of [`IDEOLOGY_NUMBERS`] present.
    pub numbers: Vec<(String, Fixed)>,
    /// Keys of [`IDEOLOGY_SWITCHES`] present.
    pub switches: Vec<(String, bool)>,
}

impl Ideology {
    pub fn rule(&self, name: &str) -> Option<bool> {
        self.rules.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }

    pub fn number(&self, key: &str) -> Option<Fixed> {
        self.numbers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    pub fn switch(&self, key: &str) -> Option<bool> {
        self.switches.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Ideologies {
    /// In definition order.
    pub list: Vec<Ideology>,
}

impl Ideologies {
    pub fn get(&self, name: &str) -> Option<&Ideology> {
        self.list.iter().find(|i| i.name == name)
    }

    /// The ideology a sub-type (`stalinism`) belongs to.
    pub fn of_type(&self, sub_type: &str) -> Option<&Ideology> {
        self.list.iter().find(|i| i.types.iter().any(|t| t.name == sub_type))
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

const DIR: &str = "common/ideologies";
const S: &str = "ideology";

pub fn load(vfs: &Vfs, report: &mut Report) -> Ideologies {
    let mut out = Ideologies::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

/// Reads one file. A second ideology with a name already defined is reported and the first kept.
pub fn parse_file(src: &Source, report: &mut Report, out: &mut Ideologies) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for entry in top.take_all("ideologies") {
        let Some(block) = ctx.block(entry, "ideologies") else { continue };
        let mut f = Fields::new(block, "ideologies");
        for e in f.take_matching(|_| true) {
            let Some(b) = ctx.block(e, S) else { continue };
            let i = parse_ideology(e, b, &mut ctx);
            if out.get(&i.name).is_some() {
                ctx.issue(e.span, S, IssueKind::Redefinition("first"));
            } else {
                out.list.push(i);
            }
        }
        f.finish(&mut ctx);
    }
    top.finish(&mut ctx);
}

fn parse_ideology(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> Ideology {
    let mut f = Fields::new(block, S);
    let mut i = Ideology { name: entry.key.text.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    if let Some(e) = f.take("types")
        && let Some(b) = ctx.block(e, "ideology.types")
    {
        let mut t = Fields::new(b, "ideology.types");
        for te in t.take_matching(|_| true) {
            let Some(tb) = ctx.block(te, "ideology.types.<type>") else { continue };
            let mut tf = Fields::new(tb, "ideology.types.<type>");
            let mut ty = IdeologyType { name: te.key.text.to_string(), can_be_randomly_selected: None };
            if let Some(c) = tf.take("can_be_randomly_selected") {
                ty.can_be_randomly_selected = ctx.bool(c, "ideology.types.<type>.can_be_randomly_selected");
            }
            tf.finish(ctx);
            i.types.push(ty);
        }
        t.finish(ctx);
    }
    if let Some(e) = f.take("dynamic_faction_names") {
        i.faction_names = ctx.names(e, "ideology.dynamic_faction_names");
    }
    if let Some(e) = f.take("color") {
        i.color = Color::from_value(&e.value);
        if i.color.is_none() {
            ctx.issue(e.value.span(), "ideology.color", IssueKind::InvalidValue("a colour"));
        }
    }
    if let Some(e) = f.take("rules")
        && let Some(b) = ctx.block(e, "ideology.rules")
    {
        let mut r = Fields::new(b, "ideology.rules");
        for re in r.take_matching(|_| true) {
            if let Some(v) = ctx.bool(re, "ideology.rules.<rule>") {
                i.rules.push((re.key.text.to_string(), v));
            }
        }
        r.finish(ctx);
    }
    if let Some(e) = f.take("modifiers")
        && let Some(b) = ctx.block(e, "ideology.modifiers")
    {
        i.modifiers = parse_modifiers(b, "ideology.modifiers", ctx);
    }
    if let Some(e) = f.take("faction_modifiers")
        && let Some(b) = ctx.block(e, "ideology.faction_modifiers")
    {
        i.faction_modifiers = parse_modifiers(b, "ideology.faction_modifiers", ctx);
    }
    for key in IDEOLOGY_NUMBERS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.fixed(e, &format!("{S}.{key}"))
        {
            i.numbers.push((key.to_string(), v));
        }
    }
    for key in IDEOLOGY_SWITCHES {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.bool(e, &format!("{S}.{key}"))
        {
            i.switches.push((key.to_string(), v));
        }
    }
    f.finish(ctx);
    i
}
