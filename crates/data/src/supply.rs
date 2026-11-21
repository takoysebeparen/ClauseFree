//! The supply network at game start: `map/railways.txt` and `map/supply_nodes.txt`.
//!
//! Both are plain lists of numbers, one record per line (the shape is read from the files themselves, see
//! `docs/CANON.md`, "Railways and supply nodes"):
//! - railways: `level count province_1 ... province_count`, a railway of that level through those provinces in order;
//! - supply nodes: `level province`.
//!
//! `#` starts a comment.

use vfs::Vfs;

use crate::provinces::ProvinceId;
use crate::report::{IssueKind, Report};
use crate::source::Source;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Railway {
    /// 1-based line in the file.
    pub line: usize,
    pub level: u32,
    /// In order along the line.
    pub provinces: Vec<ProvinceId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupplyNode {
    pub line: usize,
    pub level: u32,
    pub province: ProvinceId,
}

#[derive(Debug, Clone, Default)]
pub struct Supply {
    pub railways: Vec<Railway>,
    pub nodes: Vec<SupplyNode>,
}

pub const RAILWAYS: &str = "map/railways.txt";
pub const SUPPLY_NODES: &str = "map/supply_nodes.txt";

pub fn load(vfs: &Vfs, report: &mut Report) -> Supply {
    let mut out = Supply::default();
    if let Some(src) = Source::read(vfs, RAILWAYS, report) {
        out.railways = parse_railways(&src, report);
    }
    if let Some(src) = Source::read(vfs, SUPPLY_NODES, report) {
        out.nodes = parse_nodes(&src, report);
    }
    out
}

/// Lines of unsigned integers, comments and blank lines removed. A line with anything else is reported.
fn number_lines<'s>(src: &'s Source, schema: &'s str, report: &mut Report) -> Vec<(usize, Vec<u32>)> {
    let mut out = Vec::new();
    for (index, raw) in src.text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let numbers: Option<Vec<u32>> = line.split_whitespace().map(|t| t.parse::<u32>().ok()).collect();
        match numbers {
            Some(n) => out.push((line_no, n)),
            None => report.push(&src.path, line_no, schema, IssueKind::InvalidValue("non-negative integers")),
        }
    }
    out
}

pub fn parse_railways(src: &Source, report: &mut Report) -> Vec<Railway> {
    const S: &str = "map.railways";
    let mut out = Vec::new();
    for (line, n) in number_lines(src, S, report) {
        match n.as_slice() {
            [level, count, provinces @ ..] if *count as usize == provinces.len() && provinces.len() >= 2 => out
                .push(Railway { line, level: *level, provinces: provinces.iter().copied().map(ProvinceId).collect() }),
            _ => report.push(&src.path, line, S, IssueKind::InvalidValue("level count and that many provinces (2+)")),
        }
    }
    out
}

pub fn parse_nodes(src: &Source, report: &mut Report) -> Vec<SupplyNode> {
    const S: &str = "map.supply_nodes";
    let mut out = Vec::new();
    for (line, n) in number_lines(src, S, report) {
        match n.as_slice() {
            [level, province] => out.push(SupplyNode { line, level: *level, province: ProvinceId(*province) }),
            _ => report.push(&src.path, line, S, IssueKind::InvalidValue("level province")),
        }
    }
    out
}
