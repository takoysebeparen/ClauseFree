//! Country leader and advisor traits: `common/country_leader/*.txt` (block `leader_traits`).
//!
//! Source: the files and a key inventory of them (revision `8873ec77`, game 1.20.0.0). No shipped documentation
//! covers them. The same table holds the traits of country leaders (`dictator`), of political advisors
//! (`captain_of_industry`) and of military advisors (`army_chief_offensive_1`); characters name them in their
//! `country_leader` and `advisor` roles, and unit leader traits name them as advisor traits.
//!
//! A trait is a set of country modifiers written inline (`political_power_factor = 0.25`), plus:
//! - `random = yes | no`, `sprite = N` (the icon frame);
//! - `custom_modifier_tooltip`, `hidden_modifier = { ... }`, as in a modifier block;
//! - `targeted_modifier = { tag = X ... }` and `equipment_bonus = { ... }`, as in ideas;
//! - `ai_strategy = { type = X id = Y value = N }`;
//! - `ai_will_do`, an AI weight, deferred to the script VM.
//!
//! Values may name a constant of the file (`command_cap_increase = @tier3_cp_cap_increase`). Values are stored as
//! written.

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::ideas::{EquipmentBonus, Modifiers, TargetedModifier, parse_equipment_bonus, parse_targeted};
use crate::report::{IssueKind, Report};
use crate::source::{Constants, Ctx, Fields, Source, evaluation_order};

/// `ai_strategy = { type = X id = Y value = N }`: an AI attitude the trait gives its country.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AiStrategy {
    /// `antagonize`, `alliance`, ...
    pub kind: String,
    /// A country tag, as written; checked by the database.
    pub id: Option<String>,
    pub value: Option<Fixed>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CountryLeaderTrait {
    pub name: String,
    pub file: String,
    /// `random = yes | no`. Its effect is not documented; it is guessed to control whether randomly generated leaders
    /// may get the trait (see `docs/CANON.md`, "Country leader traits").
    pub random: Option<bool>,
    /// `sprite = N`: icon frame.
    pub sprite: Option<i64>,
    /// The inline country modifiers, custom tooltips and hidden modifiers.
    pub modifiers: Modifiers,
    pub targeted_modifiers: Vec<TargetedModifier>,
    pub equipment_bonus: Vec<EquipmentBonus>,
    pub ai_strategies: Vec<AiStrategy>,
    /// `ai_will_do = { ... }` is present (deferred to the script VM).
    pub has_ai_will_do: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CountryLeaderTraits {
    /// In definition order.
    pub list: Vec<CountryLeaderTrait>,
}

impl CountryLeaderTraits {
    pub fn get(&self, name: &str) -> Option<&CountryLeaderTrait> {
        self.list.iter().find(|t| t.name == name)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

const DIR: &str = "common/country_leader";
const T: &str = "country_leader_trait";

pub fn load(vfs: &Vfs, report: &mut Report) -> CountryLeaderTraits {
    let mut out = CountryLeaderTraits::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

/// Reads one file. A second trait with a name already defined is reported and the first kept.
pub fn parse_file(src: &Source, report: &mut Report, out: &mut CountryLeaderTraits) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    let constants = Constants::take(&mut top, &mut ctx);
    for list in top.take_all("leader_traits") {
        let Some(block) = ctx.block(list, "leader_traits") else { continue };
        let mut f = Fields::new(block, "leader_traits");
        for e in f.take_matching(|_| true) {
            let Some(b) = ctx.block(e, T) else { continue };
            let t = parse_trait(e, b, &constants, &mut ctx);
            if out.get(&t.name).is_some() {
                ctx.issue(e.span, T, IssueKind::Redefinition("first"));
            } else {
                out.list.push(t);
            }
        }
        f.finish(&mut ctx);
    }
    top.finish(&mut ctx);
}

fn parse_trait(
    entry: &Entry<'_>,
    block: &Block<'_>,
    constants: &Constants,
    ctx: &mut Ctx<'_, '_>,
) -> CountryLeaderTrait {
    let mut f = Fields::new(block, T);
    let mut t =
        CountryLeaderTrait { name: entry.key.text.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    let p = |key: &str| format!("{T}.{key}");
    t.has_ai_will_do = block.entries().any(|e| e.key.text == "ai_will_do");
    f.defer_matching(|k| k == "ai_will_do");
    if let Some(e) = f.take("random") {
        t.random = ctx.bool(e, &p("random"));
    }
    if let Some(e) = f.take("sprite") {
        t.sprite = ctx.int(e, &p("sprite"));
    }
    for e in f.take_all("targeted_modifier") {
        t.targeted_modifiers.extend(parse_targeted(e, &p("targeted_modifier"), ctx));
    }
    for e in f.take_all("equipment_bonus") {
        if let Some(b) = ctx.block(e, &p("equipment_bonus")) {
            t.equipment_bonus.extend(parse_equipment_bonus(b, &p("equipment_bonus"), ctx));
        }
    }
    for e in f.take_all("ai_strategy") {
        t.ai_strategies.extend(parse_ai_strategy(e, constants, ctx));
    }
    for e in f.take_all("custom_modifier_tooltip") {
        t.modifiers.custom_tooltips.extend(ctx.text(e, &p("custom_modifier_tooltip")));
    }
    for e in f.take_all("hidden_modifier") {
        if let Some(b) = ctx.block(e, &p("hidden_modifier")) {
            t.modifiers.hidden.extend(ctx.numbers(b, &p("hidden_modifier")));
        }
    }
    // What is left: every number (or constant) is a country modifier. Anything else is reported by `finish`.
    for e in f.take_entries(|e| e.value.as_scalar().is_some_and(|s| !s.quoted)) {
        if let Some(v) = constants.fixed(ctx, e, &p("<modifier>")) {
            t.modifiers.values.push((e.key.text.to_string(), v));
        }
    }
    f.finish(ctx);
    t
}

fn parse_ai_strategy(entry: &Entry<'_>, constants: &Constants, ctx: &mut Ctx<'_, '_>) -> Option<AiStrategy> {
    const P: &str = "country_leader_trait.ai_strategy";
    let b = ctx.block(entry, P)?;
    let mut f = Fields::new(b, P);
    let kind = f.take("type").and_then(|e| ctx.text(e, "country_leader_trait.ai_strategy.type"));
    let id = f.take("id").and_then(|e| ctx.text(e, "country_leader_trait.ai_strategy.id"));
    let value = f.take("value").and_then(|e| constants.fixed(ctx, e, "country_leader_trait.ai_strategy.value"));
    f.finish(ctx);
    match kind {
        Some(kind) => Some(AiStrategy { kind, id, value }),
        None => {
            ctx.issue(entry.span, P, IssueKind::Missing("type"));
            None
        }
    }
}
