//! Orders of battle: `history/units/*.txt`.
//!
//! An OOB file is loaded at game start when a country history runs `oob = "<file name>"`; this module reads every
//! file into an [`OobDef`] keyed by its file stem (`GER_1936`). It reads the land part (division templates and
//! divisions). Fleets, air wings, starting production (`instant_effect`) and officers are reported as deferred.
//!
//! Source: the files (key inventory of revision `7c6c1c59`, 609 files, 5074 divisions, 804 templates).

use std::collections::BTreeMap;

use clausewitz::{Block, Entry};
use common::Fixed;
use vfs::Vfs;

use crate::provinces::ProvinceId;
use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};

/// A sub-unit placed in the division designer grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateSlot {
    pub sub_unit: String,
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DivisionTemplateDef {
    pub name: String,
    pub division_names_group: Option<String>,
    pub regiments: Vec<TemplateSlot>,
    pub support: Vec<TemplateSlot>,
    pub regimental_support: Vec<TemplateSlot>,
    pub priority: Option<i64>,
    pub template_counter: Option<i64>,
    pub division_cap: Option<i64>,
    pub override_model: Option<String>,
    pub role: Option<String>,
    pub flags: Vec<(String, bool)>,
}

/// One forced equipment variant of a division: `infantry_equipment_0 = { owner = SOV version_name = "..." }`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ForcedVariant {
    pub equipment: String,
    pub owner: Option<String>,
    pub creator: Option<String>,
    pub version_name: Option<String>,
    pub amount: Option<Fixed>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DivisionDef {
    /// Explicit `name`.
    pub name: Option<String>,
    /// `division_name = { is_name_ordered = yes name_order = N }`: the N-th name of the template's name group.
    pub name_order: Option<u32>,
    pub location: Option<ProvinceId>,
    /// Template name.
    pub template: String,
    pub start_experience_factor: Option<Fixed>,
    pub start_equipment_factor: Option<Fixed>,
    pub force_equipment_variants: Vec<ForcedVariant>,
    pub bonus: Option<Fixed>,
    /// Has an `officer = { ... }` block (a character; created by the script VM).
    pub has_officer: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OobDef {
    /// File stem, the name used by `oob = "..."`.
    pub name: String,
    pub file: String,
    /// File-wide `start_equipment_factor` (a few files set it at the top level). How it combines with a division's
    /// own factor is not established yet.
    pub start_equipment_factor: Option<Fixed>,
    pub templates: Vec<DivisionTemplateDef>,
    pub divisions: Vec<DivisionDef>,
}

#[derive(Debug, Clone, Default)]
pub struct Oobs {
    pub by_name: BTreeMap<String, OobDef>,
}

impl Oobs {
    pub fn get(&self, name: &str) -> Option<&OobDef> {
        self.by_name.get(name)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

const DIR: &str = "history/units";
const T: &str = "oob.division_template";
const D: &str = "oob.division";

pub fn load(vfs: &Vfs, report: &mut Report) -> Oobs {
    let mut oobs = Oobs::default();
    for file in
        evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.to_lowercase().ends_with(".txt")).collect())
    {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        let oob = parse_file(&src, report);
        oobs.by_name.insert(oob.name.clone(), oob);
    }
    oobs
}

pub fn parse_file(src: &Source, report: &mut Report) -> OobDef {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let file_name = src.path.rsplit('/').next().unwrap_or(&src.path);
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    let mut oob = OobDef { name: stem.to_string(), file: src.path.clone(), ..Default::default() };
    let mut top = Fields::new(&parsed.root, "oob");
    if let Some(e) = top.take("start_equipment_factor") {
        oob.start_equipment_factor = ctx.fixed(e, "oob.start_equipment_factor");
    }
    for e in top.take_all("division_template") {
        let Some(b) = ctx.block(e, T) else { continue };
        if let Some(t) = parse_template(b, e, &mut ctx) {
            if oob.templates.iter().any(|x| x.name == t.name) {
                ctx.issue(e.span, T, IssueKind::Redefinition("first"));
            } else {
                oob.templates.push(t);
            }
        }
    }
    for e in top.take_all("units") {
        let Some(b) = ctx.block(e, "oob.units") else { continue };
        let mut uf = Fields::new(b, "oob.units");
        for d in uf.take_all("division") {
            let Some(db) = ctx.block(d, D) else { continue };
            oob.divisions.push(parse_division(db, &mut ctx));
        }
        // Navies are a later system.
        uf.defer_matching(|k| k == "fleet");
        uf.finish(&mut ctx);
    }
    // Air wings, starting production (script) and colonial templates (script) come later.
    top.defer_matching(|k| matches!(k, "air_wings" | "instant_effect" | "create_colonial_division_template"));
    top.finish(&mut ctx);
    oob
}

fn slots(block: &Block<'_>, schema: &str, ctx: &mut Ctx<'_, '_>) -> Vec<TemplateSlot> {
    let mut out = Vec::new();
    let mut f = Fields::new(block, schema);
    for e in f.take_matching(|_| true) {
        let Some(b) = ctx.block(e, &format!("{schema}.<sub_unit>")) else { continue };
        let mut sf = Fields::new(b, format!("{schema}.<sub_unit>"));
        let x = sf.take("x").and_then(|v| ctx.u32(v, &format!("{schema}.<sub_unit>.x")));
        let y = sf.take("y").and_then(|v| ctx.u32(v, &format!("{schema}.<sub_unit>.y")));
        sf.finish(ctx);
        match (x, y) {
            (Some(x), Some(y)) => out.push(TemplateSlot { sub_unit: e.key.text.to_string(), x, y }),
            _ => ctx.issue(e.span, format!("{schema}.<sub_unit>"), IssueKind::Missing("x and y")),
        }
    }
    f.finish(ctx);
    out
}

const TEMPLATE_FLAGS: [&str; 4] = ["is_locked", "obsolete", "force_allow_recruiting", "ingame_set_template_counter"];

fn parse_template(block: &Block<'_>, entry: &Entry<'_>, ctx: &mut Ctx<'_, '_>) -> Option<DivisionTemplateDef> {
    let mut f = Fields::new(block, T);
    let Some(name) = f.take("name").and_then(|e| ctx.text(e, &format!("{T}.name"))) else {
        ctx.issue(entry.span, T, IssueKind::Missing("name"));
        return None;
    };
    let mut t = DivisionTemplateDef { name, ..Default::default() };
    for (key, slot) in [
        ("division_names_group", &mut t.division_names_group),
        ("override_model", &mut t.override_model),
        ("role", &mut t.role),
    ] {
        if let Some(e) = f.take(key) {
            *slot = ctx.text(e, &format!("{T}.{key}"));
        }
    }
    for (key, slot) in [
        ("priority", &mut t.priority),
        ("template_counter", &mut t.template_counter),
        ("division_cap", &mut t.division_cap),
    ] {
        if let Some(e) = f.take(key) {
            *slot = ctx.int(e, &format!("{T}.{key}"));
        }
    }
    for (key, slot) in [
        ("regiments", &mut t.regiments),
        ("support", &mut t.support),
        ("regimental_support", &mut t.regimental_support),
    ] {
        for e in f.take_all(key) {
            if let Some(b) = ctx.block(e, &format!("{T}.{key}")) {
                slot.extend(slots(b, &format!("{T}.{key}"), ctx));
            }
        }
    }
    for key in TEMPLATE_FLAGS {
        if let Some(e) = f.take(key)
            && let Some(v) = ctx.bool(e, &format!("{T}.{key}"))
        {
            t.flags.push((key.to_string(), v));
        }
    }
    f.finish(ctx);
    Some(t)
}

fn parse_division(block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> DivisionDef {
    let mut f = Fields::new(block, D);
    let mut d = DivisionDef::default();
    if let Some(e) = f.take("name") {
        d.name = ctx.text(e, &format!("{D}.name"));
    }
    if let Some(e) = f.take("division_name")
        && let Some(b) = ctx.block(e, &format!("{D}.division_name"))
    {
        let mut nf = Fields::new(b, format!("{D}.division_name"));
        let ordered =
            nf.take("is_name_ordered").and_then(|v| ctx.bool(v, &format!("{D}.division_name.is_name_ordered")));
        let order = nf.take("name_order").and_then(|v| ctx.u32(v, &format!("{D}.division_name.name_order")));
        nf.finish(ctx);
        if ordered == Some(true) {
            d.name_order = order;
        }
    }
    if let Some(e) = f.take("location") {
        d.location = ctx.u32(e, &format!("{D}.location")).map(ProvinceId);
    }
    if let Some(e) = f.take("division_template") {
        d.template = ctx.text(e, &format!("{D}.division_template")).unwrap_or_default();
    }
    if d.template.is_empty() {
        ctx.issue(block.span, D, IssueKind::Missing("division_template"));
    }
    for (key, slot) in [
        ("start_experience_factor", &mut d.start_experience_factor),
        ("start_equipment_factor", &mut d.start_equipment_factor),
        ("bonus", &mut d.bonus),
    ] {
        if let Some(e) = f.take(key) {
            *slot = ctx.fixed(e, &format!("{D}.{key}"));
        }
    }
    for e in f.take_all("force_equipment_variants") {
        let Some(b) = ctx.block(e, &format!("{D}.force_equipment_variants")) else { continue };
        let schema = format!("{D}.force_equipment_variants.<equipment>");
        let mut vf = Fields::new(b, format!("{D}.force_equipment_variants"));
        for v in vf.take_matching(|_| true) {
            let Some(vb) = ctx.block(v, &schema) else { continue };
            let mut inner = Fields::new(vb, schema.clone());
            let mut fv = ForcedVariant { equipment: v.key.text.to_string(), ..Default::default() };
            for (key, slot) in
                [("owner", &mut fv.owner), ("creator", &mut fv.creator), ("version_name", &mut fv.version_name)]
            {
                if let Some(x) = inner.take(key) {
                    *slot = ctx.text(x, &format!("{schema}.{key}"));
                }
            }
            if let Some(x) = inner.take("amount") {
                fv.amount = ctx.fixed(x, &format!("{schema}.amount"));
            }
            inner.finish(ctx);
            d.force_equipment_variants.push(fv);
        }
        vf.finish(ctx);
    }
    // The officer is a character created by the script VM.
    d.has_officer = !f.take_all("officer").is_empty();
    if d.has_officer {
        ctx.issue(block.span, format!("{D}.officer"), IssueKind::Deferred);
    }
    f.finish(ctx);
    d
}
