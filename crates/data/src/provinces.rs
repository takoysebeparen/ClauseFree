//! Province definitions: `map/definition.csv` (path taken from `map/default.map`).
//!
//! Each line is `id;red;green;blue;type;coastal;terrain;continent`. The colour links a province to its pixels in
//! `provinces.bmp`. The column meanings follow the file itself; how the original treats malformed lines is unmeasured.

use std::collections::BTreeMap;

use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::Source;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProvinceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvinceKind {
    Land,
    Sea,
    Lake,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvinceDef {
    pub id: ProvinceId,
    pub color: [u8; 3],
    pub kind: ProvinceKind,
    pub coastal: bool,
    /// Terrain category key (see `TerrainCategory`), checked when the database is assembled.
    pub terrain: String,
    pub continent: u32,
}

/// All provinces, by id, plus the colour lookup used to read the province bitmap.
#[derive(Debug, Clone, Default)]
pub struct Provinces {
    pub by_id: BTreeMap<ProvinceId, ProvinceDef>,
    pub by_color: BTreeMap<[u8; 3], ProvinceId>,
}

impl Provinces {
    pub fn get(&self, id: ProvinceId) -> Option<&ProvinceDef> {
        self.by_id.get(&id)
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

const SCHEMA: &str = "map.definition";
/// Where the definitions file is named, and the name used if `default.map` does not say.
const DEFAULT_MAP: &str = "map/default.map";
const DEFAULT_DEFINITIONS: &str = "definition.csv";

/// File name of the province definitions, read from `map/default.map` (`definitions = "..."`).
pub fn definitions_path(vfs: &Vfs, report: &mut Report) -> String {
    let name = Source::read(vfs, DEFAULT_MAP, report)
        .and_then(|src| {
            let parsed = src.parse(report);
            parsed.root.get("definitions").and_then(|e| e.value.as_scalar()).map(|s| s.unescaped().into_owned())
        })
        .unwrap_or_else(|| DEFAULT_DEFINITIONS.to_string());
    format!("map/{name}")
}

/// VFS path of a map file named by `map/default.map` (`key = "file"`), or `map/<default>` if it does not say.
///
/// Problems reading `default.map` are not reported here: [`definitions_path`] already reports them once per load.
pub fn map_file(vfs: &Vfs, key: &str, default: &str) -> String {
    let name = vfs
        .read_text(DEFAULT_MAP)
        .ok()
        .and_then(|text| {
            let parsed = clausewitz::parse(&text);
            parsed.root.get(key).and_then(|e| e.value.as_scalar()).map(|s| s.unescaped().into_owned())
        })
        .unwrap_or_else(|| default.to_string());
    format!("map/{name}")
}

pub fn load(vfs: &Vfs, report: &mut Report) -> Provinces {
    let path = definitions_path(vfs, report);
    match Source::read(vfs, &path, report) {
        Some(src) => parse_definitions(&src, report),
        None => Provinces::default(),
    }
}

/// Parses the text of `definition.csv`.
pub fn parse_definitions(src: &Source, report: &mut Report) -> Provinces {
    let mut out = Provinces::default();
    for (index, raw_line) in src.text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some(def) = parse_line(line) else {
            report.push(
                &src.path,
                line_no,
                SCHEMA,
                IssueKind::InvalidValue("id;r;g;b;land|sea|lake;true|false;terrain;continent"),
            );
            continue;
        };
        if out.by_id.contains_key(&def.id) {
            report.push(&src.path, line_no, SCHEMA, IssueKind::Redefinition("first"));
            continue;
        }
        if let std::collections::btree_map::Entry::Vacant(e) = out.by_color.entry(def.color) {
            e.insert(def.id);
        } else {
            // Two provinces with one colour cannot both be found on the bitmap; keep the first.
            report.push(&src.path, line_no, format!("{SCHEMA}.color"), IssueKind::Redefinition("first"));
        }
        out.by_id.insert(def.id, def);
    }
    out
}

fn parse_line(line: &str) -> Option<ProvinceDef> {
    let cols: Vec<&str> = line.split(';').map(str::trim).collect();
    if cols.len() < 8 {
        return None;
    }
    let id = ProvinceId(cols[0].parse().ok()?);
    let color = [cols[1].parse().ok()?, cols[2].parse().ok()?, cols[3].parse().ok()?];
    let kind = match cols[4] {
        "land" => ProvinceKind::Land,
        "sea" => ProvinceKind::Sea,
        "lake" => ProvinceKind::Lake,
        _ => return None,
    };
    let coastal = match cols[5] {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    let terrain = cols[6].to_string();
    if terrain.is_empty() {
        return None;
    }
    let continent = cols[7].parse().ok()?;
    Some(ProvinceDef { id, color, kind, coastal, terrain, continent })
}
