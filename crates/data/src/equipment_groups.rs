//! Equipment groups: `common/equipment_groups/*.txt`.
//!
//! Source: `common/equipment_groups/_documentation.md` (shipped with the game). A group names equipment types,
//! archetypes, categories or other groups (`equipment_type = { ... }`) under one name, with an optional description,
//! and can be the target of an `equipment_bonus` (for example in ideas).

use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct EquipmentGroup {
    pub name: String,
    pub file: String,
    /// `description = <text or localisation key>`.
    pub description: Option<String>,
    /// `equipment_type = { ... }`: equipment, archetypes, equipment types or groups, as written.
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EquipmentGroups {
    /// In definition order.
    pub list: Vec<EquipmentGroup>,
}

impl EquipmentGroups {
    pub fn get(&self, name: &str) -> Option<&EquipmentGroup> {
        self.list.iter().find(|g| g.name == name)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

const DIR: &str = "common/equipment_groups";
const S: &str = "equipment_group";

pub fn load(vfs: &Vfs, report: &mut Report) -> EquipmentGroups {
    let mut out = EquipmentGroups::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut out);
    }
    out
}

/// Reads one file. A second group with a name already defined is reported and the first kept.
pub fn parse_file(src: &Source, report: &mut Report, out: &mut EquipmentGroups) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "");
    for entry in top.take_matching(|_| true) {
        let Some(block) = ctx.block(entry, S) else { continue };
        let mut f = Fields::new(block, S);
        let mut g = EquipmentGroup { name: entry.key.text.to_string(), file: src.path.clone(), ..Default::default() };
        if let Some(e) = f.take("description") {
            g.description = ctx.text(e, "equipment_group.description");
        }
        for e in f.take_all("equipment_type") {
            g.members.extend(ctx.names(e, "equipment_group.equipment_type"));
        }
        f.finish(&mut ctx);
        if out.get(&g.name).is_some() {
            ctx.issue(entry.span, S, IssueKind::Redefinition("first"));
        } else {
            out.list.push(g);
        }
    }
    top.finish(&mut ctx);
}
