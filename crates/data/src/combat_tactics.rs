//! Land combat tactics: `common/combat_tactics.txt` (a single file, not a directory; see `docs/CANON.md`,
//! "Combat tactics").
//!
//! The file holds `phases = { ... }` (combat phases a tactic can start; `phase = no` in a trigger is the standard
//! phase) and one block per tactic. A tactic has a side (`is_attacker`), a trigger (deferred), whether it is
//! available without unlocking (`active`), a weight (`base = { factor = N modifier = { ... } }`, the modifiers
//! deferred), a picture, the tactic that counters it, the phase it starts and the one it is shown in, the country it is
//! shown for, and its numbers (`attacker`, `defender`, `combat_width`, ...), stored as written.
//!
//! The file's own note says tactics must start with `tactic_` to work with technologies; every top-level block except
//! `phases` is read as a tactic regardless of its name.

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source};
use crate::types::{NumberList, Tag};

/// Numbers of a tactic, by key.
pub const TACTIC_NUMBERS: [&str; 6] = [
    "attacker",
    "defender",
    "combat_width",
    "attacker_movement_speed",
    "attacker_org_damage_modifier",
    "defender_org_damage_modifier",
];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CombatTactic {
    pub name: String,
    pub is_attacker: Option<bool>,
    /// `active = no` means the tactic must be unlocked (by technologies or traits).
    pub active: Option<bool>,
    /// `base = { factor = N }`.
    pub base_factor: Option<Fixed>,
    /// `base` holds `modifier = { ... }` blocks (conditional weight changes, deferred to the script VM).
    pub base_has_modifiers: bool,
    pub picture: Option<String>,
    pub countered_by: Option<String>,
    /// `phase = <phase>`: the phase this tactic starts; `no` returns to the standard phase (`tactic_cc_withdraw`).
    pub phase: Option<String>,
    /// `display_phase = <phase>`: the phase it belongs to on screen.
    pub display_phase: Option<String>,
    /// `only_show_for = <tag>`.
    pub only_show_for: Option<Tag>,
    /// Keys of [`TACTIC_NUMBERS`] present.
    pub numbers: NumberList,
    /// `trigger = { ... }` is present (deferred).
    pub has_trigger: bool,
}

impl CombatTactic {
    pub fn number(&self, key: &str) -> Option<Fixed> {
        self.numbers.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

#[derive(Debug, Clone, Default)]
pub struct CombatTactics {
    /// `phases = { ... }`.
    pub phases: Vec<String>,
    /// In definition order.
    pub list: Vec<CombatTactic>,
}

impl CombatTactics {
    pub fn get(&self, name: &str) -> Option<&CombatTactic> {
        self.list.iter().find(|t| t.name == name)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

pub const FILE: &str = "common/combat_tactics.txt";
const S: &str = "combat_tactic";

pub fn load(vfs: &Vfs, report: &mut Report) -> CombatTactics {
    let mut out = CombatTactics::default();
    if let Some(src) = Source::read(vfs, FILE, report) {
        parse_file(&src, report, &mut out);
    }
    out
}

/// Reads the file. A second tactic with a name already defined is reported and the first kept.
pub fn parse_file(src: &Source, report: &mut Report, out: &mut CombatTactics) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for e in top.take_all("phases") {
        out.phases.extend(ctx.names(e, "phases"));
    }
    for entry in top.take_matching(|_| true) {
        let Some(block) = ctx.block(entry, S) else { continue };
        let t = parse_tactic(entry, block, &mut ctx);
        if out.get(&t.name).is_some() {
            ctx.issue(entry.span, S, IssueKind::Redefinition("first"));
        } else {
            out.list.push(t);
        }
    }
    top.finish(&mut ctx);
}

fn parse_tactic(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> CombatTactic {
    let mut f = Fields::new(block, S);
    let mut t = CombatTactic { name: entry.key.text.to_string(), ..Default::default() };
    let p = |key: &str| format!("{S}.{key}");
    if let Some(e) = f.take("is_attacker") {
        t.is_attacker = ctx.bool(e, &p("is_attacker"));
    }
    if let Some(e) = f.take("active") {
        t.active = ctx.bool(e, &p("active"));
    }
    if let Some(e) = f.take("base")
        && let Some(b) = ctx.block(e, &p("base"))
    {
        let mut bf = Fields::new(b, p("base"));
        if let Some(fe) = bf.take("factor") {
            t.base_factor = ctx.fixed(fe, &p("base.factor"));
        }
        t.base_has_modifiers = !bf.take_all("modifier").is_empty();
        bf.finish(ctx);
        if t.base_has_modifiers {
            ctx.issue(b.span, p("base.modifier"), IssueKind::Deferred);
        }
    }
    for (key, slot) in [
        ("picture", &mut t.picture),
        ("countered_by", &mut t.countered_by),
        ("phase", &mut t.phase),
        ("display_phase", &mut t.display_phase),
    ] {
        if let Some(e) = f.take(key) {
            *slot = ctx.text(e, &p(key));
        }
    }
    if let Some(e) = f.take("only_show_for")
        && let Some(text) = ctx.text(e, &p("only_show_for"))
    {
        t.only_show_for = Tag::parse(&text);
        if t.only_show_for.is_none() {
            ctx.issue(e.value.span(), p("only_show_for"), IssueKind::InvalidValue("a country tag"));
        }
    }
    for key in TACTIC_NUMBERS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.fixed(e, &p(key))
        {
            t.numbers.push((key.to_string(), v));
        }
    }
    t.has_trigger = f.take("trigger").is_some();
    f.finish(ctx);
    if t.has_trigger {
        ctx.issue(entry.span, p("trigger"), IssueKind::Deferred);
    }
    t
}
