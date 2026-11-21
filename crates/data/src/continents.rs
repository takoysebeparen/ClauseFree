//! Continents: `map/continent.txt` (path taken from `map/default.map`, key `continent`).
//!
//! The file is `continents = { europe north_america ... }`. The last column of `map/definition.csv` is an index into
//! this list: `1` is the first name, `0` is used by sea provinces (see `docs/CANON.md`, "Continents").

use vfs::Vfs;

use crate::provinces::map_file;
use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Continents {
    /// Names (localisation keys) in file order; index `i` of `definition.csv` is `names[i - 1]`.
    pub names: Vec<String>,
}

impl Continents {
    /// The continent of a `definition.csv` index; `None` for `0` and for indices past the list.
    pub fn by_index(&self, index: u32) -> Option<&str> {
        let i = usize::try_from(index).ok()?.checked_sub(1)?;
        self.names.get(i).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

const S: &str = "continents";

pub fn load(vfs: &Vfs, report: &mut Report) -> Continents {
    let path = map_file(vfs, "continent", "continent.txt");
    match Source::read(vfs, &path, report) {
        Some(src) => parse_file(&src, report),
        None => Continents::default(),
    }
}

pub fn parse_file(src: &Source, report: &mut Report) -> Continents {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    let mut out = Continents::default();
    match top.take(S) {
        Some(e) => {
            // A repeated name is kept in place, because positions are what definition.csv refers to.
            for name in ctx.names(e, S) {
                if out.names.contains(&name) {
                    ctx.issue(e.span, S, IssueKind::Duplicate);
                }
                out.names.push(name);
            }
        }
        None => ctx.issue(parsed.root.span, S, IssueKind::Missing("continents")),
    }
    top.finish(&mut ctx);
    out
}
