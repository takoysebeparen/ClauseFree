//! Doctrines: `common/doctrines/{folders,grand_doctrines,tracks,subdoctrines}`.
//!
//! Source: the `_documentation.md` files shipped in each of those directories (game 1.20.0.0) and a key inventory of
//! the files (revision `8873ec77`). Concepts, as the documentation names them:
//! - a **folder** (`land`, `naval`, ...) groups grand doctrines;
//! - a **grand doctrine** is the mutually exclusive root of a folder, bought with XP; it opens its **tracks** and has
//!   one **milestone** per track ("milestones are in the same order as the tracks");
//! - a **subdoctrine** is slotted at the root of a track, bought with XP, and has **rewards** unlocked by **mastery**.
//!
//! Grand doctrines, milestones, subdoctrines and rewards share the *activation effects* ([`Effects`]): inline country
//! modifiers (or a `modifiers = { ... }` block), `enable_tactic`, `<sub-unit or category> = { <stat> = N }`,
//! `equipment_bonus` and a scripted `effect` (deferred to the script VM).
//!
//! Rewards are written as named blocks (`cavalry_charge = { ... }`), not as the anonymous blocks of the documentation's
//! example; its note says the localisation keys are `<subdoctrine>_<reward>` and `<subdoctrine>_<reward>_desc`.
//! Values are stored as written.

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::ideas::{EquipmentBonus, Modifiers, parse_equipment_bonus, parse_modifiers};
use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::technologies::{UnitStats, parse_unit_stats};

/// Activation effects of a grand doctrine, milestone, subdoctrine or reward.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Effects {
    /// Inline modifiers and the `modifiers = { ... }` block, merged in file order.
    pub modifiers: Modifiers,
    /// `enable_tactic = X`: combat tactics.
    pub tactics: Vec<String>,
    /// `<sub-unit or sub-unit category> = { ... }`.
    pub unit_stats: Vec<UnitStats>,
    pub equipment_bonus: Vec<EquipmentBonus>,
    /// `effect = { ... }` is present (deferred to the script VM).
    pub has_effect: bool,
}

impl Effects {
    pub fn unit_stats(&self, target: &str) -> Option<&UnitStats> {
        self.unit_stats.iter().find(|u| u.target == target)
    }
}

/// `mastery = { multiplier = N sub_units = { ... } categories = { ... } equipment = { ... } }`: which units gain
/// mastery for a track and how fast. On a subdoctrine it overrides the track's.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MasteryRule {
    /// "Multiplies manpower contribution to mastery gain" (documentation).
    pub multiplier: Option<Fixed>,
    pub sub_units: Vec<String>,
    pub categories: Vec<String>,
    /// Equipment types (`screen`, `capital`).
    pub equipment: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DoctrineFolder {
    pub name: String,
    pub file: String,
    /// `name = <localisation key>`.
    pub name_key: Option<String>,
    pub tab_gfx: Option<String>,
    pub color_frame: Option<i64>,
    /// `army | navy | air | all | hidden`: the intel ledger the folder shows in.
    pub ledger: Option<String>,
    pub ledger_gfx: Option<String>,
    pub sound: Option<String>,
    /// `allowed = { ... }` is present (deferred to the script VM).
    pub has_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GrandDoctrine {
    pub name: String,
    pub file: String,
    pub folder: Option<String>,
    pub name_key: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub xp_cost: Option<Fixed>,
    /// `army | navy | air`.
    pub xp_type: Option<String>,
    pub tracks: Vec<String>,
    pub max_track_rows: Option<i64>,
    pub max_track_columns: Option<i64>,
    pub effects: Effects,
    /// One per track, in the order of `tracks`.
    pub milestones: Vec<Effects>,
    /// Which of [`DOCTRINE_SCRIPTS`] the doctrine has.
    pub scripts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DoctrineTrack {
    pub name: String,
    pub file: String,
    pub name_key: Option<String>,
    pub background: Option<String>,
    pub background_offset: Option<Fixed>,
    pub frame: Option<String>,
    pub icon: Option<String>,
    pub icon_frame: Option<String>,
    pub mastery: Option<MasteryRule>,
    /// `active = { ... }` is present (deferred to the script VM).
    pub has_active: bool,
}

/// One reward of a subdoctrine.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reward {
    pub name: String,
    /// `mastery = N`: overrides `NDefines.NDoctrines.DEFAULT_REWARD_MASTERY` (documentation).
    pub mastery: Option<Fixed>,
    pub effects: Effects,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Subdoctrine {
    pub name: String,
    pub file: String,
    /// `track = X` or `track = { X Y }`.
    pub tracks: Vec<String>,
    pub allow_in_multiple_tracks: Option<bool>,
    pub name_key: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub reward_gfx: Option<String>,
    pub xp_cost: Option<Fixed>,
    pub xp_type: Option<String>,
    pub effects: Effects,
    /// In file order.
    pub rewards: Vec<Reward>,
    /// `xor = { ... }`: subdoctrines that cannot be active in another track of the same folder at the same time.
    pub xor: Vec<String>,
    pub mastery: Option<MasteryRule>,
    /// Which of [`DOCTRINE_SCRIPTS`] the subdoctrine has.
    pub scripts: Vec<String>,
}

/// Triggers and AI weights of grand doctrines and subdoctrines: understood, evaluated by the script VM.
pub const DOCTRINE_SCRIPTS: [&str; 3] = ["available", "visible", "ai_will_do"];

#[derive(Debug, Clone, Default)]
pub struct Doctrines {
    /// Each list in definition order.
    pub folders: Vec<DoctrineFolder>,
    pub grand_doctrines: Vec<GrandDoctrine>,
    pub tracks: Vec<DoctrineTrack>,
    pub subdoctrines: Vec<Subdoctrine>,
}

impl Doctrines {
    pub fn folder(&self, name: &str) -> Option<&DoctrineFolder> {
        self.folders.iter().find(|d| d.name == name)
    }

    pub fn grand_doctrine(&self, name: &str) -> Option<&GrandDoctrine> {
        self.grand_doctrines.iter().find(|d| d.name == name)
    }

    pub fn track(&self, name: &str) -> Option<&DoctrineTrack> {
        self.tracks.iter().find(|d| d.name == name)
    }

    pub fn subdoctrine(&self, name: &str) -> Option<&Subdoctrine> {
        self.subdoctrines.iter().find(|d| d.name == name)
    }

    /// Every set of activation effects: grand doctrines, milestones, subdoctrines and rewards, with the file of each.
    pub fn all_effects(&self) -> impl Iterator<Item = (&str, &Effects)> {
        let grand = self
            .grand_doctrines
            .iter()
            .flat_map(|g| std::iter::once(&g.effects).chain(&g.milestones).map(move |e| (g.file.as_str(), e)));
        let sub = self.subdoctrines.iter().flat_map(|s| {
            std::iter::once(&s.effects).chain(s.rewards.iter().map(|r| &r.effects)).map(move |e| (s.file.as_str(), e))
        });
        grand.chain(sub)
    }
}

const DIR: &str = "common/doctrines";

/// The four kinds of file, by directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Folders,
    GrandDoctrines,
    Tracks,
    Subdoctrines,
}

impl Kind {
    pub const ALL: [(Kind, &'static str); 4] = [
        (Kind::Folders, "folders"),
        (Kind::GrandDoctrines, "grand_doctrines"),
        (Kind::Tracks, "tracks"),
        (Kind::Subdoctrines, "subdoctrines"),
    ];

    fn schema(self) -> &'static str {
        match self {
            Kind::Folders => "doctrine_folder",
            Kind::GrandDoctrines => "grand_doctrine",
            Kind::Tracks => "doctrine_track",
            Kind::Subdoctrines => "subdoctrine",
        }
    }
}

/// Loads the four directories. `subdoctrines` is read with its sub-directories (`land/`, `air/`, ...); files are
/// taken in evaluation order by file name across them (an assumption, see `docs/CANON.md`, "Doctrines").
pub fn load(vfs: &Vfs, report: &mut Report) -> Doctrines {
    let mut out = Doctrines::default();
    for (kind, dir) in Kind::ALL {
        let files = vfs.list_recursive(&format!("{DIR}/{dir}"), "txt");
        for file in evaluation_order(files) {
            let Some(src) = Source::read(vfs, &file.path, report) else { continue };
            parse_file(kind, &src, report, &mut out);
        }
    }
    out
}

/// Reads one file of the given kind. A second definition with a name already used by the same kind is reported and
/// the first kept.
pub fn parse_file(kind: Kind, src: &Source, report: &mut Report, out: &mut Doctrines) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    let s = kind.schema();
    for e in top.take_matching(|_| true) {
        let Some(b) = ctx.block(e, s) else { continue };
        let name = e.key.text;
        let exists = match kind {
            Kind::Folders => out.folder(name).is_some(),
            Kind::GrandDoctrines => out.grand_doctrine(name).is_some(),
            Kind::Tracks => out.track(name).is_some(),
            Kind::Subdoctrines => out.subdoctrine(name).is_some(),
        };
        if exists {
            ctx.issue(e.span, s, IssueKind::Redefinition("first"));
            continue;
        }
        match kind {
            Kind::Folders => out.folders.push(parse_folder(e, b, &mut ctx)),
            Kind::GrandDoctrines => out.grand_doctrines.push(parse_grand(e, b, &mut ctx)),
            Kind::Tracks => out.tracks.push(parse_track(e, b, &mut ctx)),
            Kind::Subdoctrines => out.subdoctrines.push(parse_sub(e, b, &mut ctx)),
        }
    }
    top.finish(&mut ctx);
}

fn text_fields(f: &mut Fields<'_, '_>, ctx: &mut Ctx<'_, '_>, s: &str, slots: &mut [(&str, &mut Option<String>)]) {
    for (key, slot) in slots.iter_mut() {
        if let Some(e) = f.take(key) {
            **slot = ctx.text(e, &format!("{s}.{key}"));
        }
    }
}

fn scripts(block: &Block<'_>, f: &mut Fields<'_, '_>, keys: &[&str]) -> Vec<String> {
    let found = keys.iter().filter(|k| block.entries().any(|e| e.key.text == **k)).map(|k| k.to_string()).collect();
    f.defer_matching(|k| keys.contains(&k));
    found
}

fn parse_folder(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> DoctrineFolder {
    const S: &str = "doctrine_folder";
    let mut f = Fields::new(block, S);
    let mut d = DoctrineFolder { name: entry.key.text.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    d.has_allowed = !scripts(block, &mut f, &["allowed"]).is_empty();
    text_fields(
        &mut f,
        ctx,
        S,
        &mut [
            ("name", &mut d.name_key),
            ("tab_gfx", &mut d.tab_gfx),
            ("ledger", &mut d.ledger),
            ("ledger_gfx", &mut d.ledger_gfx),
            ("sound", &mut d.sound),
        ],
    );
    if let Some(e) = f.take("color_frame") {
        d.color_frame = ctx.int(e, "doctrine_folder.color_frame");
    }
    f.finish(ctx);
    d
}

fn parse_grand(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> GrandDoctrine {
    const S: &str = "grand_doctrine";
    let mut f = Fields::new(block, S);
    let mut d = GrandDoctrine { name: entry.key.text.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    d.scripts = scripts(block, &mut f, &DOCTRINE_SCRIPTS);
    text_fields(
        &mut f,
        ctx,
        S,
        &mut [
            ("folder", &mut d.folder),
            ("name", &mut d.name_key),
            ("description", &mut d.description),
            ("icon", &mut d.icon),
            ("xp_type", &mut d.xp_type),
        ],
    );
    if let Some(e) = f.take("xp_cost") {
        d.xp_cost = ctx.fixed(e, "grand_doctrine.xp_cost");
    }
    for e in f.take_all("tracks") {
        d.tracks.extend(ctx.names(e, "grand_doctrine.tracks"));
    }
    if let Some(e) = f.take("max_track_rows") {
        d.max_track_rows = ctx.int(e, "grand_doctrine.max_track_rows");
    }
    if let Some(e) = f.take("max_track_columns") {
        d.max_track_columns = ctx.int(e, "grand_doctrine.max_track_columns");
    }
    if let Some(e) = f.take("milestones")
        && let Some(b) = ctx.block(e, "grand_doctrine.milestones")
    {
        const M: &str = "grand_doctrine.milestones.<milestone>";
        let mut mf = Fields::new(b, "grand_doctrine.milestones");
        for v in mf.take_values() {
            match v.as_block() {
                Some(mb) => {
                    let mut inner = Fields::new(mb, M);
                    d.milestones.push(parse_effects(&mut inner, M, ctx));
                    inner.finish(ctx);
                }
                None => ctx.issue(v.span(), M, IssueKind::InvalidValue("a block")),
            }
        }
        mf.finish(ctx);
    }
    d.effects = parse_effects(&mut f, S, ctx);
    f.finish(ctx);
    if d.milestones.len() != d.tracks.len() {
        ctx.issue(entry.span, "grand_doctrine.milestones", IssueKind::InvalidValue("one milestone per track"));
    }
    d
}

fn parse_track(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> DoctrineTrack {
    const S: &str = "doctrine_track";
    let mut f = Fields::new(block, S);
    let mut d = DoctrineTrack { name: entry.key.text.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    d.has_active = !scripts(block, &mut f, &["active"]).is_empty();
    text_fields(
        &mut f,
        ctx,
        S,
        &mut [
            ("name", &mut d.name_key),
            ("background", &mut d.background),
            ("frame", &mut d.frame),
            ("icon", &mut d.icon),
            ("icon_frame", &mut d.icon_frame),
        ],
    );
    if let Some(e) = f.take("background_offset") {
        d.background_offset = ctx.fixed(e, "doctrine_track.background_offset");
    }
    if let Some(e) = f.take("mastery") {
        d.mastery = parse_mastery(e, "doctrine_track.mastery", ctx);
    }
    f.finish(ctx);
    d
}

fn parse_sub(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> Subdoctrine {
    const S: &str = "subdoctrine";
    let mut f = Fields::new(block, S);
    let mut d = Subdoctrine { name: entry.key.text.to_string(), file: ctx.src.path.clone(), ..Default::default() };
    d.scripts = scripts(block, &mut f, &DOCTRINE_SCRIPTS);
    for e in f.take_all("track") {
        d.tracks.extend(ctx.names(e, "subdoctrine.track"));
    }
    if let Some(e) = f.take("allow_in_multiple_tracks") {
        d.allow_in_multiple_tracks = ctx.bool(e, "subdoctrine.allow_in_multiple_tracks");
    }
    text_fields(
        &mut f,
        ctx,
        S,
        &mut [
            ("name", &mut d.name_key),
            ("description", &mut d.description),
            ("icon", &mut d.icon),
            ("reward_gfx", &mut d.reward_gfx),
            ("xp_type", &mut d.xp_type),
        ],
    );
    if let Some(e) = f.take("xp_cost") {
        d.xp_cost = ctx.fixed(e, "subdoctrine.xp_cost");
    }
    for e in f.take_all("xor") {
        d.xor.extend(ctx.names(e, "subdoctrine.xor"));
    }
    if let Some(e) = f.take("mastery") {
        d.mastery = parse_mastery(e, "subdoctrine.mastery", ctx);
    }
    if let Some(e) = f.take("rewards")
        && let Some(b) = ctx.block(e, "subdoctrine.rewards")
    {
        const R: &str = "subdoctrine.rewards.<reward>";
        let mut rf = Fields::new(b, "subdoctrine.rewards");
        for r in rf.take_matching(|_| true) {
            let Some(rb) = ctx.block(r, R) else { continue };
            if d.rewards.iter().any(|x| x.name == r.key.text) {
                ctx.issue(r.span, R, IssueKind::Redefinition("first"));
                continue;
            }
            let mut inner = Fields::new(rb, R);
            let mastery = inner.take("mastery").and_then(|m| ctx.fixed(m, "subdoctrine.rewards.<reward>.mastery"));
            let effects = parse_effects(&mut inner, R, ctx);
            inner.finish(ctx);
            d.rewards.push(Reward { name: r.key.text.to_string(), mastery, effects });
        }
        rf.finish(ctx);
    }
    if d.tracks.is_empty() {
        ctx.issue(entry.span, S, IssueKind::Missing("track"));
    }
    d.effects = parse_effects(&mut f, S, ctx);
    f.finish(ctx);
    d
}

fn parse_mastery(entry: &Entry<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Option<MasteryRule> {
    let b = ctx.block(entry, path)?;
    let mut f = Fields::new(b, path);
    let mut m = MasteryRule::default();
    if let Some(e) = f.take("multiplier") {
        m.multiplier = ctx.fixed(e, &format!("{path}.multiplier"));
    }
    for (key, slot) in
        [("sub_units", &mut m.sub_units), ("categories", &mut m.categories), ("equipment", &mut m.equipment)]
    {
        for e in f.take_all(key) {
            slot.extend(ctx.names(e, &format!("{path}.{key}")));
        }
    }
    f.finish(ctx);
    Some(m)
}

/// Reads the activation effects among the entries of `f` not taken yet. Call it after the owner's own keys are taken:
/// every remaining block is read as unit stats and every remaining number as a modifier.
fn parse_effects(f: &mut Fields<'_, '_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Effects {
    let mut out = Effects::default();
    let mut effect = Vec::new();
    for e in f.take_all("effect") {
        effect.push(e.span);
    }
    out.has_effect = !effect.is_empty();
    for span in effect {
        ctx.issue(span, format!("{path}.effect"), IssueKind::Deferred);
    }
    for e in f.take_all("enable_tactic") {
        out.tactics.extend(ctx.names(e, &format!("{path}.enable_tactic")));
    }
    // `modifiers` is the documented spelling; six rewards of `navy_capital_subdoctrines.txt` write `modifier`
    // (`modifier = { refit_speed = 0.15 }`). Both are read as modifier blocks, a guess (see `docs/CANON.md`,
    // "Doctrines").
    for e in f.take_entries(|e| e.key.text == "modifiers" || e.key.text == "modifier") {
        let p = format!("{path}.{}", e.key.text);
        if let Some(b) = ctx.block(e, &p) {
            let m = parse_modifiers(b, &p, ctx);
            out.modifiers.values.extend(m.values);
            out.modifiers.custom_tooltips.extend(m.custom_tooltips);
            out.modifiers.hidden.extend(m.hidden);
        }
    }
    for e in f.take_all("equipment_bonus") {
        let p = format!("{path}.equipment_bonus");
        if let Some(b) = ctx.block(e, &p) {
            out.equipment_bonus.extend(parse_equipment_bonus(b, &p, ctx));
        }
    }
    for e in f.take_all("custom_modifier_tooltip") {
        out.modifiers.custom_tooltips.extend(ctx.text(e, &format!("{path}.custom_modifier_tooltip")));
    }
    let unit_path = format!("{path}.<unit>");
    for e in f.take_entries(|e| e.value.as_block().is_some()) {
        if let Some(b) = e.value.as_block() {
            out.unit_stats.push(parse_unit_stats(e.key.text, b, &unit_path, ctx));
        }
    }
    for e in f.take_entries(|e| e.value.as_scalar().is_some_and(|s| !s.quoted && Fixed::parse(s.text).is_ok())) {
        if let Some(v) = ctx.fixed(e, &format!("{path}.<modifier>")) {
            out.modifiers.values.push((e.key.text.to_string(), v));
        }
    }
    out
}
