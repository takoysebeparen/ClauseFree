//! Terrain categories: `common/terrain/*.txt`, block `categories`.
//!
//! The fields are the ones present in the game files. Numbers are stored exactly as written; their meaning in combat,
//! movement and supply is defined by the systems that use them (and verified there).

use std::collections::BTreeMap;

use clausewitz::Block;
use common::Fixed;
use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::Color;

pub use crate::types::NumberList;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TerrainCategory {
    pub name: String,
    pub color: Option<Color>,
    pub is_water: bool,
    pub naval_terrain: bool,
    pub movement_cost: Option<Fixed>,
    pub combat_width: Option<Fixed>,
    pub combat_support_width: Option<Fixed>,
    pub ai_terrain_importance_factor: Option<Fixed>,
    pub match_value: Option<Fixed>,
    pub sound_type: Option<String>,
    pub supply_flow_penalty_factor: Option<Fixed>,
    pub truck_attrition_factor: Option<Fixed>,
    pub attrition: Option<Fixed>,
    pub sickness_chance: Option<Fixed>,
    pub minimum_seazone_dominance: Option<Fixed>,
    pub naval_mine_hit_chance: Option<Fixed>,
    pub enemy_army_bonus_air_superiority_factor: Option<Fixed>,
    pub positioning: Option<Fixed>,
    pub navy_visibility: Option<Fixed>,
    /// `buildings_max_level = { bunker = 4 }`.
    pub buildings_max_level: NumberList,
    /// `units = { attack = -0.15 }`: modifiers for land units fighting in this terrain.
    pub unit_modifiers: NumberList,
    /// Per unit-type modifiers, e.g. `submarine = { navy_visibility = ... }`. The type names are checked against
    /// unit types once those are loaded.
    pub unit_type_modifiers: BTreeMap<String, UnitTypeModifiers>,
}

/// Modifiers for one unit type in a terrain: direct values, plus an optional `units = { ... }` block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UnitTypeModifiers {
    pub values: NumberList,
    pub units: NumberList,
}

#[derive(Debug, Clone, Default)]
pub struct Terrain {
    /// Categories in definition order.
    pub categories: Vec<TerrainCategory>,
}

impl Terrain {
    pub fn get(&self, name: &str) -> Option<&TerrainCategory> {
        self.categories.iter().find(|c| c.name == name)
    }
}

const DIR: &str = "common/terrain";

pub fn load(vfs: &Vfs, report: &mut Report) -> Terrain {
    let mut terrain = Terrain::default();
    for file in evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_file(&src, report, &mut terrain);
    }
    terrain
}

pub fn parse_file(src: &Source, report: &mut Report, terrain: &mut Terrain) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "terrain");
    for entry in top.take_all("categories") {
        let Some(block) = ctx.block(entry, "terrain.categories") else { continue };
        for category in block.entries() {
            let Some(body) = ctx.block(category, "terrain.categories.<category>") else { continue };
            let parsed = parse_category(category.key.text, body, &mut ctx);
            if terrain.get(&parsed.name).is_some() {
                // Unmeasured which definition the original keeps; keep the first and report.
                ctx.issue(category.span, "terrain.category", IssueKind::Redefinition("first"));
            } else {
                terrain.categories.push(parsed);
            }
        }
    }
    // Graphical terrain types for terrain.bmp belong to the renderer.
    top.defer_matching(|k| k == "terrain" || k == "graphical_terrain" || k == "ambient_object");
    top.finish(&mut ctx);
}

fn number_list(block: &Block<'_>, schema: &str, ctx: &mut Ctx<'_, '_>) -> NumberList {
    ctx.numbers(block, schema)
}

fn parse_category(name: &str, block: &Block<'_>, ctx: &mut Ctx<'_, '_>) -> TerrainCategory {
    const S: &str = "terrain.category";
    let mut f = Fields::new(block, S);
    let mut c = TerrainCategory { name: name.to_string(), ..Default::default() };
    if let Some(e) = f.take("color") {
        c.color = Color::from_value(&e.value);
        if c.color.is_none() {
            ctx.issue(e.span, f.path("color"), IssueKind::InvalidValue("a colour"));
        }
    }
    if let Some(e) = f.take("is_water") {
        c.is_water = ctx.bool(e, &f.path("is_water")).unwrap_or(false);
    }
    if let Some(e) = f.take("naval_terrain") {
        c.naval_terrain = ctx.bool(e, &f.path("naval_terrain")).unwrap_or(false);
    }
    if let Some(e) = f.take("sound_type") {
        c.sound_type = ctx.text(e, &f.path("sound_type"));
    }
    let numbers: [(&str, &mut Option<Fixed>); 14] = [
        ("movement_cost", &mut c.movement_cost),
        ("combat_width", &mut c.combat_width),
        ("combat_support_width", &mut c.combat_support_width),
        ("ai_terrain_importance_factor", &mut c.ai_terrain_importance_factor),
        ("match_value", &mut c.match_value),
        ("supply_flow_penalty_factor", &mut c.supply_flow_penalty_factor),
        ("truck_attrition_factor", &mut c.truck_attrition_factor),
        ("attrition", &mut c.attrition),
        ("sickness_chance", &mut c.sickness_chance),
        ("minimum_seazone_dominance", &mut c.minimum_seazone_dominance),
        ("naval_mine_hit_chance", &mut c.naval_mine_hit_chance),
        ("enemy_army_bonus_air_superiority_factor", &mut c.enemy_army_bonus_air_superiority_factor),
        ("positioning", &mut c.positioning),
        ("navy_visibility", &mut c.navy_visibility),
    ];
    for (key, slot) in numbers {
        if let Some(e) = f.take(key) {
            *slot = ctx.fixed(e, &f.path(key));
        }
    }
    if let Some(e) = f.take("buildings_max_level")
        && let Some(b) = ctx.block(e, &f.path("buildings_max_level"))
    {
        c.buildings_max_level = number_list(b, &f.path("buildings_max_level"), ctx);
    }
    if let Some(e) = f.take("units")
        && let Some(b) = ctx.block(e, &f.path("units"))
    {
        c.unit_modifiers = number_list(b, &f.path("units"), ctx);
    }
    // Any remaining block is a per-unit-type modifier block (`submarine = { ... }`).
    for e in f.take_matching(|_| true) {
        match e.value.as_block() {
            Some(b) => {
                let schema = format!("{S}.<unit type>");
                let mut tf = Fields::new(b, schema.clone());
                let mut modifiers = UnitTypeModifiers::default();
                if let Some(u) = tf.take("units")
                    && let Some(ub) = ctx.block(u, &format!("{schema}.units"))
                {
                    modifiers.units = number_list(ub, &format!("{schema}.units"), ctx);
                }
                for v in tf.take_matching(|_| true) {
                    if let Some(n) = ctx.fixed(v, &format!("{schema}.{}", v.key.text)) {
                        modifiers.values.push((v.key.text.to_string(), n));
                    }
                }
                tf.finish(ctx);
                c.unit_type_modifiers.insert(e.key.text.to_string(), modifiers);
            }
            None => ctx.issue(e.span, format!("{S}.{}", e.key.text), IssueKind::UnknownKey),
        }
    }
    f.finish(ctx);
    c
}
