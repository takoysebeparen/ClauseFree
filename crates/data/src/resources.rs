//! Strategic resources: `common/resources/*.txt`, block `resources`.
//!
//! The order of definitions matters in the original (see `common/resources/_documentation.md`, which ties an AI
//! define to it), so it is kept.

use common::Fixed;
use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResourceDef {
    pub name: String,
    /// Frame in the resource icon strip.
    pub icon_frame: Option<i64>,
    /// Civilian factory output bought by one unit of trade (`cic`).
    pub cic: Option<Fixed>,
    /// Convoys needed per unit traded.
    pub convoys: Option<Fixed>,
}

#[derive(Debug, Clone, Default)]
pub struct Resources {
    /// In definition order.
    pub list: Vec<ResourceDef>,
}

impl Resources {
    pub fn get(&self, name: &str) -> Option<&ResourceDef> {
        self.list.iter().find(|r| r.name == name)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

const DIR: &str = "common/resources";

pub fn load(vfs: &Vfs, report: &mut Report) -> Resources {
    let mut out = Resources::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

pub fn parse_file(src: &Source, report: &mut Report, out: &mut Resources) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for list in top.take_all("resources") {
        let Some(block) = ctx.block(list, "resources") else { continue };
        for entry in block.entries() {
            let Some(body) = ctx.block(entry, "resources.<resource>") else { continue };
            let mut f = Fields::new(body, "resources.<resource>");
            let mut r = ResourceDef { name: entry.key.text.to_string(), ..Default::default() };
            if let Some(e) = f.take("icon_frame") {
                r.icon_frame = ctx.int(e, "resources.<resource>.icon_frame");
            }
            if let Some(e) = f.take("cic") {
                r.cic = ctx.fixed(e, "resources.<resource>.cic");
            }
            if let Some(e) = f.take("convoys") {
                r.convoys = ctx.fixed(e, "resources.<resource>.convoys");
            }
            f.finish(&mut ctx);
            if out.get(&r.name).is_some() {
                ctx.issue(entry.span, "resources.<resource>", IssueKind::Redefinition("first"));
            } else {
                out.list.push(r);
            }
        }
    }
    top.finish(&mut ctx);
}
