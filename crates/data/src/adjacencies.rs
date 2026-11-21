//! Extra connections between provinces: `map/adjacencies.csv` and the rules they may name, `map/adjacency_rules.txt`
//! (both paths taken from `map/default.map`).
//!
//! Ordinary neighbours come from the province bitmap; this file adds sea crossings (`sea`, with the sea province
//! crossed in `Through`), removes borders (`impassable`) and attaches rules (straits, canals) to a pair of provinces.
//! The columns are named by the file's own header: `From;To;Type;Through;start_x;start_y;stop_x;stop_y;
//! adjacency_rule_name;Comment`. The rule semantics are described by the comment at the top of
//! `adjacency_rules.txt`; see `docs/CANON.md`, "Adjacencies".

use clausewitz::{Block, Entry};
use vfs::Vfs;

use crate::provinces::{ProvinceId, map_file};
use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjacencyKind {
    /// The `Type` column is empty: the provinces are neighbours anyway; the line attaches a rule (straits).
    Unspecified,
    /// `sea`: a crossing over the `Through` sea province.
    Sea,
    /// `impassable`: the border between the two provinces cannot be crossed.
    Impassable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adjacency {
    /// 1-based line in the file.
    pub line: usize,
    pub from: ProvinceId,
    pub to: ProvinceId,
    pub kind: AdjacencyKind,
    /// `Through` (`-1` means none).
    pub through: Option<ProvinceId>,
    /// `start_x;start_y` and `stop_x;stop_y` in map pixels, for drawing; `-1` means none.
    pub start: Option<[i32; 2]>,
    pub stop: Option<[i32; 2]>,
    /// `adjacency_rule_name` (empty means none).
    pub rule: Option<String>,
    /// `Comment` as written; not used by the game as far as known.
    pub comment: String,
}

/// What may pass in one situation (`contested`, `enemy`, `friend`, `neutral`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Passage {
    pub army: bool,
    pub navy: bool,
    pub submarine: bool,
    pub trade: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AdjacencyRule {
    pub name: String,
    pub file: String,
    pub contested: Passage,
    pub enemy: Passage,
    pub friend: Passage,
    pub neutral: Passage,
    /// Provinces that make up the passage (who controls them decides the situation).
    pub required_provinces: Vec<ProvinceId>,
    /// Province where the rule's icon is drawn, and its offset `{ x y z }`.
    pub icon: Option<ProvinceId>,
    pub offset: Option<[common::Fixed; 3]>,
    /// Trigger blocks present (`is_disabled`, `is_friend`, `is_enemy`, `is_neutral`); run by the script VM.
    pub triggers: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Adjacencies {
    /// In file order.
    pub list: Vec<Adjacency>,
    /// In definition order.
    pub rules: Vec<AdjacencyRule>,
}

impl Adjacencies {
    pub fn rule(&self, name: &str) -> Option<&AdjacencyRule> {
        self.rules.iter().find(|r| r.name == name)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

const CSV: &str = "map.adjacencies";
const RULE: &str = "adjacency_rule";
const TRIGGERS: [&str; 4] = ["is_disabled", "is_friend", "is_enemy", "is_neutral"];
const SITUATIONS: [&str; 4] = ["contested", "enemy", "friend", "neutral"];

pub fn load(vfs: &Vfs, report: &mut Report) -> Adjacencies {
    let mut out = Adjacencies::default();
    if let Some(src) = Source::read(vfs, &map_file(vfs, "adjacencies", "adjacencies.csv"), report) {
        out.list = parse_csv(&src, report);
    }
    if let Some(src) = Source::read(vfs, &map_file(vfs, "adjacency_rules", "adjacency_rules.txt"), report) {
        parse_rules(&src, report, &mut out);
    }
    out
}

/// Parses `adjacencies.csv`.
///
/// Lines starting with `#` are comments. The first other line is a header if its first column is not a number.
/// A line whose `From` is `-1` ends the list; anything but blank lines after it is reported.
pub fn parse_csv(src: &Source, report: &mut Report) -> Vec<Adjacency> {
    let mut out = Vec::new();
    let mut first = true;
    let mut ended = false;
    for (index, raw) in src.text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if ended {
            report.push(&src.path, line_no, CSV, IssueKind::InvalidValue("nothing after the -1 line"));
            continue;
        }
        let cols: Vec<&str> = line.split(';').map(str::trim).collect();
        if std::mem::take(&mut first) && cols[0].parse::<i64>().is_err() {
            continue;
        }
        if cols[0] == "-1" {
            ended = true;
            continue;
        }
        match parse_line(&cols, line_no) {
            Some(a) => out.push(a),
            None => report.push(
                &src.path,
                line_no,
                CSV,
                IssueKind::InvalidValue("from;to;|sea|impassable;through;x;y;x;y;rule;comment"),
            ),
        }
    }
    out
}

fn parse_line(cols: &[&str], line: usize) -> Option<Adjacency> {
    if cols.len() < 9 {
        return None;
    }
    let province = |s: &str| s.parse::<u32>().ok().map(ProvinceId);
    let optional =
        |s: &str| -> Option<Option<ProvinceId>> { if s == "-1" { Some(None) } else { province(s).map(Some) } };
    let point = |x: &str, y: &str| -> Option<Option<[i32; 2]>> {
        let (x, y) = (x.parse::<i32>().ok()?, y.parse::<i32>().ok()?);
        Some(if x == -1 && y == -1 { None } else { Some([x, y]) })
    };
    let kind = match cols[2] {
        "" => AdjacencyKind::Unspecified,
        "sea" => AdjacencyKind::Sea,
        "impassable" => AdjacencyKind::Impassable,
        _ => return None,
    };
    Some(Adjacency {
        line,
        from: province(cols[0])?,
        to: province(cols[1])?,
        kind,
        through: optional(cols[3])?,
        start: point(cols[4], cols[5])?,
        stop: point(cols[6], cols[7])?,
        rule: Some(cols[8]).filter(|r| !r.is_empty()).map(str::to_string),
        comment: cols.get(9..).map(|rest| rest.join(";")).unwrap_or_default(),
    })
}

/// Parses `adjacency_rules.txt`: `adjacency_rule = { ... }` blocks. A second rule with a name already defined is
/// reported and the first kept.
pub fn parse_rules(src: &Source, report: &mut Report, out: &mut Adjacencies) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for entry in top.take_all(RULE) {
        let Some(block) = ctx.block(entry, RULE) else { continue };
        let Some(rule) = parse_rule(entry, block, &mut ctx) else { continue };
        if out.rule(&rule.name).is_some() {
            ctx.issue(entry.span, RULE, IssueKind::Redefinition("first"));
        } else {
            out.rules.push(rule);
        }
    }
    top.finish(&mut ctx);
}

fn parse_rule(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> Option<AdjacencyRule> {
    let mut f = Fields::new(block, RULE);
    let mut r = AdjacencyRule { file: ctx.src.path.clone(), ..Default::default() };
    let name = f.take("name").and_then(|e| ctx.text(e, "adjacency_rule.name"));
    for (i, situation) in SITUATIONS.iter().enumerate() {
        let path = format!("{RULE}.{situation}");
        let passage = match f.take(situation) {
            Some(e) => ctx.block(e, &path).map(|b| parse_passage(b, &path, ctx)).unwrap_or_default(),
            None => {
                ctx.issue(entry.span, &path, IssueKind::Missing("situation block"));
                Passage::default()
            }
        };
        match i {
            0 => r.contested = passage,
            1 => r.enemy = passage,
            2 => r.friend = passage,
            _ => r.neutral = passage,
        }
    }
    if let Some(e) = f.take("required_provinces")
        && let Some(b) = ctx.block(e, "adjacency_rule.required_provinces")
    {
        r.required_provinces =
            ctx.u32_list(b, "adjacency_rule.required_provinces").into_iter().map(ProvinceId).collect();
    }
    if let Some(e) = f.take("icon") {
        r.icon = ctx.u32(e, "adjacency_rule.icon").map(ProvinceId);
    }
    if let Some(e) = f.take("offset") {
        r.offset = offset(e, ctx);
    }
    for key in TRIGGERS {
        if f.take(key).is_some() {
            r.triggers.push(key.to_string());
        }
    }
    f.finish(ctx);
    for key in &r.triggers {
        // Triggers run in the script VM.
        ctx.issue(entry.span, format!("{RULE}.{key}"), IssueKind::Deferred);
    }
    match name {
        Some(name) => {
            r.name = name;
            Some(r)
        }
        None => {
            ctx.issue(entry.span, RULE, IssueKind::Missing("name"));
            None
        }
    }
}

fn parse_passage(block: &Block<'_>, path: &str, ctx: &mut Ctx<'_, '_>) -> Passage {
    let mut f = Fields::new(block, path);
    let mut flag = |key: &str, ctx: &mut Ctx<'_, '_>| -> bool {
        match f.take(key) {
            Some(e) => ctx.bool(e, &format!("{path}.{key}")).unwrap_or(false),
            None => {
                ctx.issue(block.span, format!("{path}.{key}"), IssueKind::Missing("yes or no"));
                false
            }
        }
    };
    let p = Passage {
        army: flag("army", ctx),
        navy: flag("navy", ctx),
        submarine: flag("submarine", ctx),
        trade: flag("trade", ctx),
    };
    f.finish(ctx);
    p
}

fn offset(entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>) -> Option<[common::Fixed; 3]> {
    const P: &str = "adjacency_rule.offset";
    let block = ctx.block(entry, P)?;
    let values: Vec<common::Fixed> =
        block.values().filter_map(|v| v.as_scalar()).filter_map(|s| common::Fixed::parse(s.text).ok()).collect();
    match (values.as_slice(), block.entries().next(), block.values().count()) {
        ([x, y, z], None, 3) => Some([*x, *y, *z]),
        _ => {
            ctx.issue(block.span, P, IssueKind::InvalidValue("{ x y z }"));
            None
        }
    }
}
