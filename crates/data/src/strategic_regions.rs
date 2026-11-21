//! Strategic regions: `map/strategicregions/*.txt`.
//!
//! Each file holds `strategic_region = { id name provinces naval_terrain static_modifiers weather }`. Regions group
//! provinces for air and naval operations and carry the weather. A weather `period` covers `between = { d.m d.m }`,
//! a zero-based day and month (see `docs/CANON.md`, "Strategic regions"), with a temperature range and phenomenon
//! weights stored as written.

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::provinces::ProvinceId;
use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::NumberList;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionId(pub u32);

/// A day of the year as written in `between`: zero-based day of month and zero-based month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DayOfYear {
    pub month: u8,
    pub day: u8,
}

impl DayOfYear {
    /// Reads `d.m` (`0.0` is 1 January, `27.1` is 28 February). The text is split, not read as a number, because
    /// `0.10` (November) and `0.1` (February) differ.
    pub fn parse(text: &str) -> Option<DayOfYear> {
        let (day, month) = text.split_once('.')?;
        let (day, month) = (day.parse::<u8>().ok()?, month.parse::<u8>().ok()?);
        (month < 12 && day < 31).then_some(DayOfYear { month, day })
    }
}

/// Phenomenon weights and limits of a weather period, in the order the files use.
pub const WEATHER_KEYS: [&str; 9] = [
    "no_phenomenon",
    "rain_light",
    "rain_heavy",
    "snow",
    "blizzard",
    "arctic_water",
    "mud",
    "sandstorm",
    "min_snow_level",
];

#[derive(Debug, Clone, PartialEq)]
pub struct WeatherPeriod {
    pub from: DayOfYear,
    pub to: DayOfYear,
    /// `temperature = { min max }`.
    pub temperature: [Fixed; 2],
    /// The keys of [`WEATHER_KEYS`] present, as written.
    pub values: NumberList,
}

impl WeatherPeriod {
    pub fn get(&self, key: &str) -> Option<Fixed> {
        self.values.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrategicRegion {
    pub id: RegionId,
    pub file: String,
    /// `name = <localisation key>`.
    pub name: Option<String>,
    pub provinces: Vec<ProvinceId>,
    /// `naval_terrain = <terrain category>` (sea regions).
    pub naval_terrain: Option<String>,
    /// `static_modifiers = { <static modifier> = <value> }`, as written (the files use `always`).
    pub static_modifiers: Vec<(String, String)>,
    pub weather: Vec<WeatherPeriod>,
}

#[derive(Debug, Clone, Default)]
pub struct StrategicRegions {
    pub by_id: std::collections::BTreeMap<RegionId, StrategicRegion>,
}

impl StrategicRegions {
    pub fn get(&self, id: RegionId) -> Option<&StrategicRegion> {
        self.by_id.get(&id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

const DIR: &str = "map/strategicregions";
const S: &str = "strategic_region";

pub fn load(vfs: &Vfs, report: &mut Report) -> StrategicRegions {
    let mut out = StrategicRegions::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

/// Reads one file. A second region with an id already defined is reported and the first kept.
pub fn parse_file(src: &Source, report: &mut Report, out: &mut StrategicRegions) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for entry in top.take_all(S) {
        let Some(block) = ctx.block(entry, S) else { continue };
        let Some(region) = parse_region(entry, block, &mut ctx) else { continue };
        match out.by_id.entry(region.id) {
            std::collections::btree_map::Entry::Occupied(_) => {
                ctx.issue(entry.span, S, IssueKind::Redefinition("first"));
            }
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(region);
            }
        }
    }
    top.finish(&mut ctx);
}

fn parse_region(entry: &Entry<'_>, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> Option<StrategicRegion> {
    let mut f = Fields::new(block, S);
    let id = f.take("id").and_then(|e| ctx.u32(e, "strategic_region.id"));
    let mut r = StrategicRegion {
        id: RegionId(0),
        file: ctx.src.path.clone(),
        name: None,
        provinces: Vec::new(),
        naval_terrain: None,
        static_modifiers: Vec::new(),
        weather: Vec::new(),
    };
    if let Some(e) = f.take("name") {
        r.name = ctx.text(e, "strategic_region.name");
    }
    if let Some(e) = f.take("provinces")
        && let Some(b) = ctx.block(e, "strategic_region.provinces")
    {
        r.provinces = ctx.u32_list(b, "strategic_region.provinces").into_iter().map(ProvinceId).collect();
    }
    if let Some(e) = f.take("naval_terrain") {
        r.naval_terrain = ctx.text(e, "strategic_region.naval_terrain");
    }
    if let Some(e) = f.take("static_modifiers")
        && let Some(b) = ctx.block(e, "strategic_region.static_modifiers")
    {
        let mut m = Fields::new(b, "strategic_region.static_modifiers");
        for e in m.take_matching(|_| true) {
            if let Some(v) = ctx.text(e, "strategic_region.static_modifiers.<modifier>") {
                r.static_modifiers.push((e.key.text.to_string(), v));
            }
        }
        m.finish(ctx);
    }
    if let Some(e) = f.take("weather")
        && let Some(b) = ctx.block(e, "strategic_region.weather")
    {
        let mut w = Fields::new(b, "strategic_region.weather");
        for p in w.take_all("period") {
            r.weather.extend(ctx.block(p, "strategic_region.weather.period").and_then(|b| period(b, ctx)));
        }
        w.finish(ctx);
    }
    f.finish(ctx);
    match id {
        Some(id) => {
            r.id = RegionId(id);
            Some(r)
        }
        None => {
            ctx.issue(entry.span, S, IssueKind::Missing("id"));
            None
        }
    }
}

fn period(block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> Option<WeatherPeriod> {
    const P: &str = "strategic_region.weather.period";
    let mut f = Fields::new(block, P);
    let between_entry = f.take("between");
    let temperature_entry = f.take("temperature");
    let missing = |name: &'static str, present: bool, ctx: &mut Ctx<'_, '_>| {
        if !present {
            ctx.issue(block.span, P, IssueKind::Missing(name));
        }
    };
    missing("between", between_entry.is_some(), ctx);
    missing("temperature", temperature_entry.is_some(), ctx);
    let between = between_entry.and_then(|e| {
        let b = ctx.block(e, &format!("{P}.between"))?;
        let days: Vec<Option<DayOfYear>> =
            b.values().map(|v| v.as_scalar().and_then(|s| DayOfYear::parse(s.text))).collect();
        match (days.as_slice(), b.entries().next()) {
            ([Some(from), Some(to)], None) => Some((*from, *to)),
            _ => {
                ctx.issue(b.span, format!("{P}.between"), IssueKind::InvalidValue("{ d.m d.m }"));
                None
            }
        }
    });
    let temperature = temperature_entry.and_then(|e| {
        let b = ctx.block(e, &format!("{P}.temperature"))?;
        let values: Vec<Option<Fixed>> =
            b.values().map(|v| v.as_scalar().and_then(|s| Fixed::parse(s.text).ok())).collect();
        match (values.as_slice(), b.entries().next()) {
            ([Some(lo), Some(hi)], None) => Some([*lo, *hi]),
            _ => {
                ctx.issue(b.span, format!("{P}.temperature"), IssueKind::InvalidValue("{ min max }"));
                None
            }
        }
    });
    let mut values = Vec::new();
    for key in WEATHER_KEYS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.fixed(e, &format!("{P}.{key}"))
        {
            values.push((key.to_string(), v));
        }
    }
    f.finish(ctx);
    let (from, to) = between?;
    Some(WeatherPeriod { from, to, temperature: temperature?, values })
}
