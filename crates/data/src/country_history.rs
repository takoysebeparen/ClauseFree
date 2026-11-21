//! Country history: `history/countries/*.txt`, one file per country, named `<TAG> - <name>.txt`.
//!
//! The whole file is an effect block run for the country at game start, with dated blocks (`1939.1.1 = { ... }`)
//! run at their date. This module reads the *plain* top-level effects that set up the start (capital, OOB, research
//! slots, technologies, politics, ideas, characters, ...) as data; everything else (`if`, variables, scopes, other
//! effects) is deferred to the script VM, like the history of states. Because effects run in order, a plain value
//! written twice is taken from the last one (the effects documentation describes `set_oob` as "replacing any
//! previously registered OOB"), and an `if` elsewhere in the file may still change it when the script VM runs.
//!
//! The tag is the first three characters of the file name. Source: every base game file starts with the tag, two of
//! them with an irregular separator (`HBC  - Hebei-Chahar.txt`, `KLT -Kalat.txt`); see `docs/CANON.md`,
//! "Country history".
//!
//! Effect shapes: `documentation/effects_documentation.md` (`set_politics`, `set_popularities`, `set_oob`,
//! `set_research_slots`, `recruit_character`, `set_grand_doctrine`, `set_sub_doctrine`, `set_fuel_ratio`).

use std::collections::BTreeMap;

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::states::StateId;
use crate::types::{Date, NumberList, Tag};

/// `set_politics = { ruling_party = X last_election = D election_frequency = N elections_allowed = yes }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Politics {
    pub ruling_party: Option<String>,
    pub last_election: Option<Date>,
    /// Months between elections.
    pub election_frequency: Option<i64>,
    pub elections_allowed: Option<bool>,
    /// `name` / `long_name`: localisation keys that rename the ruling party.
    pub name: Option<String>,
    pub long_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CountryHistory {
    pub tag: Option<Tag>,
    pub file: String,
    pub capital: Option<StateId>,
    /// `oob` / `set_oob`: the OOB file (stem) loaded at game start.
    pub oob: Option<String>,
    pub research_slots: Option<i64>,
    pub convoys: Option<i64>,
    pub stability: Option<Fixed>,
    pub war_support: Option<Fixed>,
    pub fuel_ratio: Option<Fixed>,
    pub starting_train_buffer: Option<Fixed>,
    /// `set_technology = { <tech> = 1 }`, every block in order; `popup = no` is not a technology and is dropped.
    pub technologies: NumberList,
    pub politics: Option<Politics>,
    /// `set_popularities = { <ideology> = N }` (the last one; the documentation says unnamed ideologies get 0).
    pub popularities: NumberList,
    /// `add_ideas`, in order.
    pub ideas: Vec<String>,
    /// `recruit_character`, in order.
    pub characters: Vec<String>,
    pub grand_doctrines: Vec<String>,
    pub sub_doctrines: Vec<String>,
    /// Dated blocks, applied by the script VM at their date. Only the dates are kept.
    pub dated: Vec<Date>,
}

#[derive(Debug, Clone, Default)]
pub struct CountryHistories {
    pub by_tag: BTreeMap<Tag, CountryHistory>,
}

impl CountryHistories {
    pub fn get(&self, tag: Tag) -> Option<&CountryHistory> {
        self.by_tag.get(&tag)
    }

    pub fn len(&self) -> usize {
        self.by_tag.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_tag.is_empty()
    }
}

const DIR: &str = "history/countries";
const S: &str = "country_history";

/// The tag a history file belongs to: the first three characters of its name.
pub fn tag_of_file(path: &str) -> Option<Tag> {
    let name = path.rsplit('/').next().unwrap_or(path);
    Tag::parse(name.get(..3)?)
}

pub fn load(vfs: &Vfs, report: &mut Report) -> CountryHistories {
    let mut out = CountryHistories::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

/// Reads one file. A second file for a tag that already has one is reported and the first kept (evaluation order).
pub fn parse_file(src: &Source, report: &mut Report, out: &mut CountryHistories) {
    let Some(tag) = tag_of_file(&src.path) else {
        report.push(&src.path, 0, S, IssueKind::InvalidValue("a file name starting with a country tag"));
        return;
    };
    if out.by_tag.contains_key(&tag) {
        report.push(&src.path, 0, S, IssueKind::Redefinition("first"));
        return;
    }
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let h = parse_history(tag, &parsed.root, &mut ctx);
    out.by_tag.insert(tag, h);
}

/// The last of several entries (effects run in order), each read with `read`.
fn last<T>(entries: Vec<&Entry<'_>>, mut read: impl FnMut(&Entry<'_>) -> Option<T>) -> Option<T> {
    entries.into_iter().filter_map(&mut read).last()
}

fn parse_history(tag: Tag, root: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> CountryHistory {
    let mut f = Fields::new(root, S);
    let mut h = CountryHistory { tag: Some(tag), file: ctx.src.path.clone(), ..Default::default() };
    h.capital = last(f.take_all("capital"), |e| ctx.u32(e, "country_history.capital").map(StateId));
    // `OOB` (upper case) is written in two base game files; read as `oob` (see `docs/CANON.md`, "Country history").
    let oobs = f.take_entries(|e| matches!(e.key.text, "oob" | "OOB" | "set_oob"));
    h.oob = last(oobs, |e| ctx.text(e, "country_history.oob"));
    h.research_slots = last(f.take_all("set_research_slots"), |e| ctx.int(e, "country_history.set_research_slots"));
    h.convoys = last(f.take_all("set_convoys"), |e| ctx.int(e, "country_history.set_convoys"));
    h.starting_train_buffer =
        last(f.take_all("starting_train_buffer"), |e| ctx.fixed(e, "country_history.starting_train_buffer"));
    h.stability = last(f.take_all("set_stability"), |e| ctx.fixed(e, "country_history.set_stability"));
    h.war_support = last(f.take_all("set_war_support"), |e| ctx.fixed(e, "country_history.set_war_support"));
    h.fuel_ratio = last(f.take_all("set_fuel_ratio"), |e| ctx.fixed(e, "country_history.set_fuel_ratio"));
    for e in f.take_all("set_technology") {
        const P: &str = "country_history.set_technology";
        let Some(b) = ctx.block(e, P) else { continue };
        let mut tf = Fields::new(b, P);
        // `popup = no`: do not show the research notification (one file).
        if let Some(p) = tf.take("popup") {
            ctx.bool(p, "country_history.set_technology.popup");
        }
        for t in tf.take_matching(|_| true) {
            if let Some(v) = ctx.fixed(t, "country_history.set_technology.<technology>") {
                h.technologies.push((t.key.text.to_string(), v));
            }
        }
        tf.finish(ctx);
    }
    h.politics = last(f.take_all("set_politics"), |e| parse_politics(e, ctx));
    h.popularities = last(f.take_all("set_popularities"), |e| {
        const P: &str = "country_history.set_popularities";
        let b = ctx.block(e, P)?;
        // Values may be variables set earlier in the file (`democratic = PER_democratic_popularity`); then the result
        // is only known to the script VM, and the plain value is unknown (empty).
        let numeric = |e: &Entry<'_>| e.value.as_scalar().is_some_and(|s| !s.quoted && Fixed::parse(s.text).is_ok());
        if b.entries().any(|e| e.value.as_scalar().is_some() && !numeric(e)) {
            ctx.issue(e.span, "country_history.set_popularities (variables)", IssueKind::Deferred);
            return Some(Vec::new());
        }
        Some(ctx.numbers(b, P))
    })
    .unwrap_or_default();
    for e in f.take_all("add_ideas") {
        h.ideas.extend(ctx.names(e, "country_history.add_ideas"));
    }
    for (key, slot) in [
        ("recruit_character", &mut h.characters),
        ("set_grand_doctrine", &mut h.grand_doctrines),
        ("set_sub_doctrine", &mut h.sub_doctrines),
    ] {
        for e in f.take_all(key) {
            slot.extend(ctx.text(e, &format!("{S}.{key}")));
        }
    }
    for e in f.take_matching(|k| Date::parse(k).is_some()) {
        if let Some(date) = Date::parse(e.key.text) {
            h.dated.push(date);
        }
        ctx.issue(e.span, "country_history.<date>", IssueKind::Deferred);
    }
    // The rest are effects, conditions and scope changes, run by the script VM.
    f.defer_matching(|_| true);
    f.finish(ctx);
    h
}

fn parse_politics(entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>) -> Option<Politics> {
    const P: &str = "country_history.set_politics";
    let b = ctx.block(entry, P)?;
    let mut f = Fields::new(b, P);
    let mut p = Politics::default();
    for (key, slot) in [("ruling_party", &mut p.ruling_party), ("name", &mut p.name), ("long_name", &mut p.long_name)] {
        if let Some(e) = f.take(key) {
            *slot = ctx.text(e, &format!("{P}.{key}"));
        }
    }
    if let Some(e) = f.take("last_election")
        && let Some(text) = ctx.text(e, "country_history.set_politics.last_election")
    {
        p.last_election = Date::parse(&text);
        if p.last_election.is_none() {
            ctx.issue(e.value.span(), "country_history.set_politics.last_election", IssueKind::InvalidValue("a date"));
        }
    }
    if let Some(e) = f.take("election_frequency") {
        p.election_frequency = ctx.int(e, "country_history.set_politics.election_frequency");
    }
    if let Some(e) = f.take("elections_allowed") {
        p.elections_allowed = ctx.bool(e, "country_history.set_politics.elections_allowed");
    }
    f.finish(ctx);
    Some(p)
}
