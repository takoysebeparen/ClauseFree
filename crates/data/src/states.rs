//! States: `history/states/*.txt`, one or more `state = { ... }` per file.
//!
//! The static part (provinces, manpower, category, resources) and the plain start-of-game history (owner,
//! controller, cores, claims, victory points, buildings) are read here. Everything else inside `history` is script
//! (effects such as `set_variable`, `if`, dated blocks) and is *deferred* to the script VM.
//!
//! Duplicate state ids: the first evaluated definition is kept. Source: community wiki, "Modding", section "Loading
//! files" (example with `123-ABC.txt` / `123-XYZ.txt`). Files are evaluated in ASCII file-name order.

use std::collections::BTreeMap;

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::provinces::ProvinceId;
use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::{Date, NumberList, Tag};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct StateId(pub u32);

/// One building at the start of the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartBuilding {
    pub building: String,
    pub level: i64,
    /// The building has an `allowed = { ... }` trigger (for example a DLC check); evaluated by the script VM.
    pub conditional: bool,
}

/// Buildings at the start: state-wide levels and per-province levels.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StartBuildings {
    pub state: Vec<StartBuilding>,
    pub provinces: BTreeMap<ProvinceId, Vec<StartBuilding>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StateHistory {
    pub owner: Option<Tag>,
    pub controller: Option<Tag>,
    pub cores: Vec<Tag>,
    pub claims: Vec<Tag>,
    /// `victory_points = { province points }`; points are fixed-point (fractions occur).
    pub victory_points: Vec<(ProvinceId, Fixed)>,
    pub buildings: StartBuildings,
    /// Dated history blocks (`1939.1.1 = { ... }`), applied by the script VM at their date. Only the dates are kept.
    pub dated: Vec<Date>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StateDef {
    pub id: StateId,
    /// Localisation key of the name (`STATE_1`).
    pub name: String,
    pub manpower: Option<i64>,
    pub category: Option<String>,
    pub provinces: Vec<ProvinceId>,
    pub resources: NumberList,
    pub local_supplies: Option<Fixed>,
    pub buildings_max_level_factor: Option<Fixed>,
    pub impassable: bool,
    /// `force_link_ownership_to = <state>`: this state always changes hands with that one.
    pub force_link_ownership_to: Option<StateId>,
    /// `impassable_ignored_links = { <state> ... }`.
    pub impassable_ignored_links: Vec<StateId>,
    pub history: StateHistory,
    /// VFS path of the defining file.
    pub file: String,
}

#[derive(Debug, Clone, Default)]
pub struct States {
    pub by_id: BTreeMap<StateId, StateDef>,
}

impl States {
    pub fn get(&self, id: StateId) -> Option<&StateDef> {
        self.by_id.get(&id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

const DIR: &str = "history/states";
const S: &str = "state";
const H: &str = "state.history";

pub fn load(vfs: &Vfs, report: &mut Report) -> States {
    let mut states = States::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut states);
    }
    states
}

pub fn parse_file(src: &Source, report: &mut Report, states: &mut States) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for entry in top.take_all("state") {
        let Some(block) = ctx.block(entry, S) else { continue };
        let Some(state) = parse_state(block, entry, &mut ctx) else { continue };
        if states.by_id.contains_key(&state.id) {
            ctx.issue(entry.span, S, IssueKind::Redefinition("first"));
            continue;
        }
        states.by_id.insert(state.id, state);
    }
    top.finish(&mut ctx);
}

fn tag_value(entry: &Entry<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Option<Tag> {
    let s = ctx.scalar(entry, path)?;
    let tag = Tag::parse(s.text);
    if tag.is_none() {
        ctx.issue(s.span, path, IssueKind::InvalidValue("a country tag"));
    }
    tag
}

fn parse_state(block: &Block<'_>, entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>) -> Option<StateDef> {
    let mut f = Fields::new(block, S);
    let Some(id) = f.take("id").and_then(|e| ctx.u32(e, "state.id")) else {
        // Without an id the state is unusable; its other keys are not reported separately.
        ctx.issue(entry.span, S, IssueKind::Missing("id"));
        return None;
    };
    let mut state = StateDef { id: StateId(id), file: ctx.src.path.clone(), ..Default::default() };
    if let Some(e) = f.take("name") {
        state.name = ctx.text(e, "state.name").unwrap_or_default();
    }
    if let Some(e) = f.take("manpower") {
        state.manpower = ctx.int(e, "state.manpower");
    }
    if let Some(e) = f.take("state_category") {
        state.category = ctx.text(e, "state.state_category");
    }
    if let Some(e) = f.take("provinces")
        && let Some(b) = ctx.block(e, "state.provinces")
    {
        state.provinces = ctx.u32_list(b, "state.provinces").into_iter().map(ProvinceId).collect();
    }
    if let Some(e) = f.take("resources")
        && let Some(b) = ctx.block(e, "state.resources")
    {
        let mut rf = Fields::new(b, "state.resources");
        for r in rf.take_matching(|_| true) {
            if let Some(v) = ctx.fixed(r, "state.resources.<resource>") {
                state.resources.push((r.key.text.to_string(), v));
            }
        }
        rf.finish(ctx);
    }
    if let Some(e) = f.take("local_supplies") {
        state.local_supplies = ctx.fixed(e, "state.local_supplies");
    }
    if let Some(e) = f.take("buildings_max_level_factor") {
        state.buildings_max_level_factor = ctx.fixed(e, "state.buildings_max_level_factor");
    }
    if let Some(e) = f.take("force_link_ownership_to") {
        state.force_link_ownership_to = ctx.u32(e, "state.force_link_ownership_to").map(StateId);
    }
    if let Some(e) = f.take("impassable_ignored_links")
        && let Some(b) = ctx.block(e, "state.impassable_ignored_links")
    {
        state.impassable_ignored_links =
            ctx.u32_list(b, "state.impassable_ignored_links").into_iter().map(StateId).collect();
    }
    if let Some(e) = f.take("impassable") {
        state.impassable = ctx.bool(e, "state.impassable").unwrap_or(false);
    }
    if let Some(e) = f.take("history")
        && let Some(b) = ctx.block(e, H)
    {
        state.history = parse_history(b, ctx);
    }
    f.finish(ctx);
    Some(state)
}

fn parse_history(block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> StateHistory {
    let mut f = Fields::new(block, H);
    let mut h = StateHistory::default();
    if let Some(e) = f.take("owner") {
        h.owner = tag_value(e, "state.history.owner", ctx);
    }
    if let Some(e) = f.take("controller") {
        h.controller = tag_value(e, "state.history.controller", ctx);
    }
    for e in f.take_all("add_core_of") {
        h.cores.extend(tag_value(e, "state.history.add_core_of", ctx));
    }
    for e in f.take_all("add_claim_by") {
        h.claims.extend(tag_value(e, "state.history.add_claim_by", ctx));
    }
    for e in f.take_all("victory_points") {
        let Some(b) = ctx.block(e, "state.history.victory_points") else { continue };
        let parts: Vec<&str> = b.values().filter_map(|v| v.as_str()).collect();
        match (parts.as_slice(), b.entries().next()) {
            ([province, points], None) => match (province.parse::<u32>(), Fixed::parse(points)) {
                (Ok(p), Ok(v)) => h.victory_points.push((ProvinceId(p), v)),
                _ => ctx.issue(b.span, "state.history.victory_points", IssueKind::InvalidValue("{ province points }")),
            },
            _ => ctx.issue(b.span, "state.history.victory_points", IssueKind::InvalidValue("{ province points }")),
        }
    }
    for e in f.take_all("buildings") {
        let Some(b) = ctx.block(e, "state.history.buildings") else { continue };
        parse_buildings(b, &mut h.buildings, ctx);
    }
    for e in f.take_matching(|k| Date::parse(k).is_some()) {
        if let Some(date) = Date::parse(e.key.text) {
            h.dated.push(date);
        }
        // Dated blocks hold effects; they run in the script VM.
        ctx.issue(e.span, "state.history.<date>", IssueKind::Deferred);
    }
    // The remaining keys of `history` are effects (the block is an effect scope). Report them as deferred to the
    // script VM; their names are listed by `documentation/effects_documentation.md`.
    f.defer_matching(|_| true);
    f.finish(ctx);
    h
}

/// `name = level` or `name = { level = N allowed = { <trigger> } }`.
fn building(entry: &Entry<'_>, schema: &str, ctx: &mut Ctx<'_, '_>) -> Option<StartBuilding> {
    let name = entry.key.text.to_string();
    match entry.value.as_block() {
        None => ctx.int(entry, schema).map(|level| StartBuilding { building: name, level, conditional: false }),
        Some(block) => {
            let mut f = Fields::new(block, schema);
            let level = f.take("level").and_then(|e| ctx.int(e, &format!("{schema}.level")));
            let conditional = f.take("allowed").is_some();
            f.finish(ctx);
            if conditional {
                // The trigger itself is evaluated by the script VM.
                ctx.issue(entry.span, format!("{schema}.allowed"), IssueKind::Deferred);
            }
            match level {
                Some(level) => Some(StartBuilding { building: name, level, conditional }),
                None => {
                    ctx.issue(entry.span, schema, IssueKind::Missing("level"));
                    None
                }
            }
        }
    }
}

fn parse_buildings(block: &Block<'_>, out: &mut StartBuildings, ctx: &mut Ctx<'_, '_>) {
    const B: &str = "state.history.buildings";
    let mut f = Fields::new(block, B);
    for e in f.take_matching(|_| true) {
        match e.key.text.parse::<u32>() {
            Ok(province) => {
                let Some(pb) = ctx.block(e, "state.history.buildings.<province>") else { continue };
                let mut pf = Fields::new(pb, "state.history.buildings.<province>");
                let mut list = Vec::new();
                for be in pf.take_matching(|_| true) {
                    list.extend(building(be, "state.history.buildings.<province>.<building>", ctx));
                }
                pf.finish(ctx);
                out.provinces.entry(ProvinceId(province)).or_default().extend(list);
            }
            Err(_) => out.state.extend(building(e, "state.history.buildings.<building>", ctx)),
        }
    }
    f.finish(ctx);
}
