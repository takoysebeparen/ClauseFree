use std::path::Path;

use common::Fixed;
use data::adjacencies::{self, Adjacencies, AdjacencyKind};
use data::combat_tactics::{self, CombatTactics};
use data::continents;
use data::ideologies::{self, Ideologies};
use data::provinces::ProvinceId;
use data::report::{IssueKind, Report};
use data::source::Source;
use data::strategic_regions::{self, DayOfYear, RegionId, StrategicRegions};
use data::supply;
use data::unit_leader::{self, UnitLeaders};
use data::{Color, Database, LoadOptions, Tag};

fn fixed(text: &str) -> Fixed {
    Fixed::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn src(path: &str, text: &str) -> Source {
    Source::from_text(path, text.to_string())
}

#[test]
fn continents_are_indexed_from_one() {
    let mut report = Report::default();
    let c = continents::parse_file(&src("map/continent.txt", "continents = { europe asia europe }\n"), &mut report);
    assert_eq!(c.len(), 3, "a repeated name keeps its position");
    assert_eq!(
        (c.by_index(0), c.by_index(1), c.by_index(2), c.by_index(4)),
        (None, Some("europe"), Some("asia"), None)
    );
    assert_eq!(report.count(&IssueKind::Duplicate), 1);
}

const ADJACENCIES: &str = "From;To;Type;Through;start_x;start_y;stop_x;stop_y;adjacency_rule_name;Comment\n\
    12251;10321;sea;8313;-1;-1;-1;-1;;Andaman\n\
    # a comment line\n\
    8857;2284;sea;7617;1560;1009;1556;1016;PANAMA_CANAL;Panama;with;semicolons\n\
    5407;5382;;-1;-1;-1;-1;-1;GIBRALTAR_STRAIT;Gibraltar North\n\
    1;2;impassable;-1;-1;-1;-1;-1;;\n\
    3;4;canal;-1;-1;-1;-1;-1;;bad type\n\
    5;x;sea;6;-1;-1;-1;-1;;bad province\n\
    -1;-1;;-1;-1;-1;-1;-1;-1\n\
    \n\
    7;8;sea;9;-1;-1;-1;-1;;after the end\n";

const RULES: &str = r#"
adjacency_rule = {
	name = "SUEZ_CANAL"
	contested = { army = no navy = no submarine = no trade = no }
	enemy = { army = no navy = no submarine = yes trade = no }
	friend = { army = yes navy = yes submarine = yes trade = yes }
	neutral = { army = yes navy = yes submarine = yes trade = yes }
	required_provinces = { 12049 1155 }
	is_disabled = { has_global_flag = SUEZ_CANAL_BLOCKED tooltip = suez_blocked_tt }
	is_friend = { always = yes }
	icon = 12049
	offset = { 1 0 -6 }
	unknown_thing = 1
}
adjacency_rule = { name = "SUEZ_CANAL" contested = { army = yes navy = yes submarine = yes trade = yes } }
adjacency_rule = { contested = { army = yes } }
"#;

#[test]
fn adjacencies_and_rules() {
    let mut report = Report::default();
    let list = adjacencies::parse_csv(&src("map/adjacencies.csv", ADJACENCIES), &mut report);
    assert_eq!(list.len(), 4, "header, comment, two bad lines, terminator and the line after it are not adjacencies");
    let andaman = &list[0];
    assert_eq!(
        (andaman.from, andaman.to, andaman.kind, andaman.through, andaman.start, andaman.rule.as_deref()),
        (ProvinceId(12251), ProvinceId(10321), AdjacencyKind::Sea, Some(ProvinceId(8313)), None, None)
    );
    assert_eq!(andaman.line, 2);
    let panama = &list[1];
    assert_eq!((panama.start, panama.stop), (Some([1560, 1009]), Some([1556, 1016])));
    assert_eq!((panama.rule.as_deref(), panama.comment.as_str()), (Some("PANAMA_CANAL"), "Panama;with;semicolons"));
    assert_eq!((list[2].kind, list[2].through), (AdjacencyKind::Unspecified, None));
    assert_eq!(list[3].kind, AdjacencyKind::Impassable);
    let invalid: Vec<usize> =
        report.issues.iter().filter(|i| matches!(i.kind, IssueKind::InvalidValue(_))).map(|i| i.line).collect();
    assert_eq!(invalid, vec![7, 8, 11], "bad type, bad province, a line after the terminator");

    let mut report = Report::default();
    let mut out = Adjacencies::default();
    adjacencies::parse_rules(&src("map/adjacency_rules.txt", RULES), &mut report, &mut out);
    assert_eq!(out.rules.len(), 1);
    let suez = out.rule("SUEZ_CANAL").expect("suez");
    assert!(!suez.contested.army && suez.enemy.submarine && !suez.enemy.navy && suez.friend.trade);
    assert_eq!(suez.required_provinces, vec![ProvinceId(12049), ProvinceId(1155)]);
    assert_eq!((suez.icon, suez.offset), (Some(ProvinceId(12049)), Some([fixed("1"), fixed("0"), fixed("-6")])));
    assert_eq!(suez.triggers, vec!["is_disabled".to_string(), "is_friend".to_string()]);
    assert_eq!(report.count(&IssueKind::Deferred), 2);
    assert_eq!(report.count(&IssueKind::UnknownKey), 1);
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);
    assert_eq!(report.count(&IssueKind::Missing("name")), 1);
    // The second rule lacks three situations, the third lacks three situations and three flags.
    assert_eq!(report.count(&IssueKind::Missing("situation block")), 6);
    assert_eq!(report.count(&IssueKind::Missing("yes or no")), 3);
}

const REGION: &str = r#"
strategic_region={
	id=75
	name="STRATEGICREGION_75"
	provinces={ 1 2 3 }
	naval_terrain=water_shallow_sea
	static_modifiers={ water_modifier_shark_infested=always }
	weather={
		period={
			between={ 0.1 27.1 }
			temperature={ -7.0 12.0 }
			no_phenomenon=0.500
			snow=0.200
			min_snow_level=0.000
		}
		period={
			between={ 0.10 29.10 }
			temperature={ 1.0 }
			mud=0.3
		}
		period={ temperature={ 1.0 2.0 } fog=1 }
	}
}
strategic_region={ id=75 provinces={ 4 } }
strategic_region={ provinces={ 5 } }
"#;

#[test]
fn strategic_regions_and_weather() {
    assert_eq!(DayOfYear::parse("0.10"), Some(DayOfYear { month: 10, day: 0 }), "November, not February");
    assert_eq!(DayOfYear::parse("27.1"), Some(DayOfYear { month: 1, day: 27 }));
    assert_eq!((DayOfYear::parse("0.12"), DayOfYear::parse("1")), (None, None));

    let mut report = Report::default();
    let mut out = StrategicRegions::default();
    strategic_regions::parse_file(&src("map/strategicregions/75.txt", REGION), &mut report, &mut out);
    assert_eq!(out.len(), 1);
    let r = out.get(RegionId(75)).expect("75");
    assert_eq!(r.provinces, vec![ProvinceId(1), ProvinceId(2), ProvinceId(3)], "the redefinition is not merged");
    assert_eq!(
        (r.name.as_deref(), r.naval_terrain.as_deref()),
        (Some("STRATEGICREGION_75"), Some("water_shallow_sea"))
    );
    assert_eq!(r.static_modifiers, vec![("water_modifier_shark_infested".to_string(), "always".to_string())]);
    assert_eq!(r.weather.len(), 1, "the other periods are malformed");
    let feb = &r.weather[0];
    assert_eq!((feb.from, feb.to), (DayOfYear { month: 1, day: 0 }, DayOfYear { month: 1, day: 27 }));
    assert_eq!(feb.temperature, [fixed("-7"), fixed("12")]);
    assert_eq!(
        (feb.get("snow"), feb.get("min_snow_level"), feb.get("mud")),
        (Some(fixed("0.2")), Some(fixed("0")), None)
    );
    assert_eq!(report.count(&IssueKind::InvalidValue("{ min max }")), 1);
    assert_eq!(report.count(&IssueKind::Missing("between")), 1);
    assert_eq!(report.count(&IssueKind::UnknownKey), 1, "fog");
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);
    assert_eq!(report.count(&IssueKind::Missing("id")), 1);
}

#[test]
fn railways_and_supply_nodes() {
    let mut report = Report::default();
    let text = "3 4 6521 11444 3312 6282 \n#1 3 6747 723 3707\n2 3 1 2\n1 1 5\n1 2 7 x\n";
    let rails = supply::parse_railways(&src("map/railways.txt", text), &mut report);
    assert_eq!(rails.len(), 1);
    assert_eq!((rails[0].level, rails[0].line, rails[0].provinces.len()), (3, 1, 4));
    assert_eq!(rails[0].provinces[3], ProvinceId(6282));
    let mut lines: Vec<usize> = report.issues.iter().map(|i| i.line).collect();
    lines.sort();
    assert_eq!(lines, vec![3, 4, 5], "count mismatch, one province, text");

    let mut report = Report::default();
    let nodes = supply::parse_nodes(&src("map/supply_nodes.txt", "1 67 \n1 101\n2\n"), &mut report);
    assert_eq!(nodes.iter().map(|n| (n.level, n.province.0)).collect::<Vec<_>>(), vec![(1, 67), (1, 101)]);
    assert_eq!(report.issues.len(), 1);
}

const IDEOLOGIES: &str = r#"
ideologies = {
	democratic = {
		types = {
			conservatism = { }
			buddhist_socialism = { can_be_randomly_selected = no }
		}
		dynamic_faction_names = { "FACTION_NAME_DEMOCRATIC_1" "FACTION_NAME_DEMOCRATIC_2" }
		color = { 0 0 255 }
		rules = { can_puppet = no can_force_government = yes }
		can_host_government_in_exile = yes
		war_impact_on_world_tension = 0.25
		modifiers = { generate_wargoal_tension = 1.00 hidden_modifier = { join_faction_tension = -0.1 } }
		faction_modifiers = { faction_trade_opinion_factor = 0.50 }
		ai_democratic = yes
		mystery = 1
	}
	democratic = { }
}
"#;

#[test]
fn ideologies_with_types_and_rules() {
    let mut report = Report::default();
    let mut out = Ideologies::default();
    ideologies::parse_file(&src("common/ideologies/00_ideologies.txt", IDEOLOGIES), &mut report, &mut out);
    assert_eq!(out.len(), 1);
    let d = out.get("democratic").expect("democratic");
    assert_eq!(d.types.len(), 2);
    assert_eq!(d.types[1].can_be_randomly_selected, Some(false));
    assert_eq!(out.of_type("buddhist_socialism").map(|i| i.name.as_str()), Some("democratic"));
    assert_eq!(d.faction_names.len(), 2);
    assert_eq!(d.color, Some(Color::Rgb([0, 0, 255])));
    assert_eq!((d.rule("can_puppet"), d.rule("can_force_government"), d.rule("x")), (Some(false), Some(true), None));
    assert_eq!(d.number("war_impact_on_world_tension"), Some(fixed("0.25")));
    assert_eq!((d.switch("ai_democratic"), d.switch("can_host_government_in_exile")), (Some(true), Some(true)));
    assert_eq!(d.modifiers.get("generate_wargoal_tension"), Some(fixed("1")));
    assert_eq!(d.modifiers.hidden, vec![("join_faction_tension".to_string(), fixed("-0.1"))]);
    assert_eq!(d.faction_modifiers.get("faction_trade_opinion_factor"), Some(fixed("0.5")));
    assert_eq!(report.count(&IssueKind::UnknownKey), 1);
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);
}

const TACTICS: &str = r#"
phases = { close_combat street_fighting }
tactic_basic_attack = {
	is_attacker = yes
	trigger = { is_attacker = yes phase = no }
	active = yes
	base = { factor = 4 modifier = { add = 4 has_trait = unyielding_defender } }
	picture = attack
	countered_by = tactic_counterattack
	attacker = 0.05
	combat_width = -0.25
}
tactic_breakthrough = {
	is_attacker = yes
	active = no
	base = { factor = 4 }
	phase = close_combat
	display_phase = close_combat
	only_show_for = SOV
	defender_org_damage_modifier = 0.1
}
tactic_breakthrough = { }
tactic_bad = { only_show_for = soviet }
"#;

#[test]
fn combat_tactics_and_phases() {
    let mut report = Report::default();
    let mut out = CombatTactics::default();
    combat_tactics::parse_file(&src(combat_tactics::FILE, TACTICS), &mut report, &mut out);
    assert_eq!(out.phases, vec!["close_combat".to_string(), "street_fighting".to_string()]);
    assert_eq!(out.len(), 3);
    let basic = out.get("tactic_basic_attack").expect("basic");
    assert_eq!((basic.is_attacker, basic.active, basic.base_factor), (Some(true), Some(true), Some(fixed("4"))));
    assert!(basic.base_has_modifiers && basic.has_trigger);
    assert_eq!(
        (basic.picture.as_deref(), basic.countered_by.as_deref()),
        (Some("attack"), Some("tactic_counterattack"))
    );
    assert_eq!((basic.number("attacker"), basic.number("combat_width")), (Some(fixed("0.05")), Some(fixed("-0.25"))));
    let b = out.get("tactic_breakthrough").expect("breakthrough");
    assert_eq!(
        (b.active, b.phase.as_deref(), b.display_phase.as_deref()),
        (Some(false), Some("close_combat"), Some("close_combat"))
    );
    assert_eq!(b.only_show_for, Tag::parse("SOV"));
    assert_eq!(b.number("defender_org_damage_modifier"), Some(fixed("0.1")));
    assert!(!b.has_trigger && !b.base_has_modifiers);
    assert_eq!(report.count(&IssueKind::Deferred), 2, "the trigger and the base modifiers");
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);
    assert_eq!(report.count(&IssueKind::InvalidValue("a country tag")), 1);
}

const TRAITS: &str = r#"
leader_traits = {
	organizer = { type = corps_commander trait_type = basic_trait }
	desert_fox = {
		type = { corps_commander field_marshal }
		trait_type = assignable_terrain_trait
		attack_skill = 1
		planning_skill_factor = 0.5
		modifier = {
			out_of_supply_factor = -0.1
			desert = { movement = 0.1 }
		}
		field_marshal_modifier = { max_dig_in_factor = 0.30 }
		sub_unit_modifiers = { destroyer = { naval_damage_factor = 0.1 } }
		trait_xp_factor = { organizer = 1.5 }
		any_parent = { organizer }
		parent = { traits = { organizer brilliant_strategist } num_parents_needed = 2 }
		all_parents = { organizer brilliant_strategist }
		mutually_exclusive = thorough_planner
		gui_row = 5
		gui_column = -1
		cost = 1000
		show_in_combat = yes
		unit_type = { type = cavalry type = camelry }
		allowed_ship_equipments = { ship_hull_submarine }
		enable_ability = force_attack
		slot = army_chief
		specialist_advisor_trait = army_chief_offensive_1
		custom_effect_tooltip = some_tt
		allowed = { always = yes }
		ai_will_do = { factor = 1 }
		strange = yes
	}
	organizer = { }
}
leader_attack_skills = {
	1 = {
		cost = 100
		type = navy
		modifier = { naval_damage_factor = 0.05 }
		building_module_modifier = { spotting_chance = 0.02 }
	}
	1 = { type = navy }
	2 = { cost = 200 }
	x = { }
}
"#;

#[test]
fn leader_traits_and_skills() {
    let mut report = Report::default();
    let mut out = UnitLeaders::default();
    unit_leader::parse_file(&src("common/unit_leader/00_traits.txt", TRAITS), &mut report, &mut out);
    assert_eq!(out.traits.len(), 2);
    let t = out.get_trait("desert_fox").expect("desert_fox");
    assert_eq!(t.types, vec!["corps_commander".to_string(), "field_marshal".to_string()]);
    assert_eq!(t.trait_type.as_deref(), Some("assignable_terrain_trait"));
    assert_eq!((t.skill("attack_skill"), t.skill("planning_skill_factor")), (Some(fixed("1")), Some(fixed("0.5"))));
    let m = t.modifiers("modifier").expect("modifier");
    assert_eq!(m.get("out_of_supply_factor"), Some(fixed("-0.1")));
    assert_eq!(m.terrain, vec![("desert".to_string(), vec![("movement".to_string(), fixed("0.1"))])]);
    assert_eq!(t.modifiers("field_marshal_modifier").and_then(|m| m.get("max_dig_in_factor")), Some(fixed("0.3")));
    assert_eq!(t.sub_unit_modifiers[0].0, "destroyer");
    assert_eq!(t.trait_xp_factor, vec![("organizer".to_string(), fixed("1.5"))]);
    assert_eq!(
        t.parents.iter().map(|p| p.needed).collect::<Vec<_>>(),
        vec![2, 1, 2],
        "parent, any_parent, all_parents"
    );
    assert_eq!(t.mutually_exclusive, vec!["thorough_planner".to_string()]);
    assert_eq!(
        (t.gui_row, t.gui_column, t.cost, t.show_in_combat),
        (Some(5), Some(-1), Some(fixed("1000")), Some(true))
    );
    assert_eq!(t.unit_types, vec!["cavalry".to_string(), "camelry".to_string()]);
    assert_eq!((t.allowed_ship_equipments.len(), t.abilities.len()), (1, 1));
    assert_eq!(t.advisor_slot.as_deref(), Some("army_chief"));
    assert_eq!(t.advisor_traits, vec![("specialist_advisor_trait".to_string(), "army_chief_offensive_1".to_string())]);
    assert_eq!(t.tooltips.len(), 1);
    assert_eq!(t.scripts, vec!["allowed".to_string(), "ai_will_do".to_string()]);
    assert_eq!(out.skills.len(), 2, "level 1 and level 2 (the redefinition of 1 is not kept)");
    let one = out.skill_level("attack", "navy", 1).expect("attack 1");
    assert_eq!((one.cost, one.modifiers.get("naval_damage_factor")), (Some(fixed("100")), Some(fixed("0.05"))));
    assert_eq!(one.building_module_modifiers.get("spotting_chance"), Some(fixed("0.02")));
    assert_eq!(report.count(&IssueKind::Deferred), 2);
    assert_eq!(report.count(&IssueKind::UnknownKey), 1, "strange");
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 2, "trait and skill level");
    assert_eq!(report.count(&IssueKind::Missing("type")), 1, "level 2");
    assert_eq!(report.count(&IssueKind::InvalidValue("a level number")), 1);
}

/// The real installation (skipped without `HOI4_DIR`).
#[test]
fn real_map_topology_and_military() {
    let Ok(dir) = std::env::var("HOI4_DIR") else { return };
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let (db, report) = Database::load(&vfs, &LoadOptions::default());
    assert_eq!(db.continents.by_index(1), Some("europe"));
    assert!(db.adjacencies.len() > 200, "{}", db.adjacencies.len());
    assert!(db.adjacencies.rule("SUEZ_CANAL").is_some());
    assert!(db.strategic_regions.len() > 300, "{}", db.strategic_regions.len());
    let england = db.strategic_regions.get(RegionId(1)).expect("region 1");
    assert_eq!(england.weather.len(), 12);
    assert!(db.supply.railways.len() > 900 && db.supply.nodes.len() > 700);
    assert_eq!(db.ideologies.len(), 4);
    assert_eq!(db.ideologies.of_type("stalinism").map(|i| i.name.as_str()), Some("communism"));
    assert!(db.combat_tactics.len() > 50);
    assert!(db.combat_tactics.get("tactic_basic_attack").is_some_and(|t| t.active == Some(true)));
    assert!(db.unit_leaders.traits.len() > 150);
    assert!(db.unit_leaders.skill_level("attack", "corps_commander", 1).is_some());
    let prefixes = [
        "map.adjacencies",
        "adjacency_rule",
        "strategic_region",
        "map.railways",
        "map.supply_nodes",
        "continents",
        "ideology",
        "combat_tactic",
        "phases",
        "leader_trait",
        "leader_",
    ];
    for prefix in prefixes {
        let unknown: Vec<String> = report
            .issues
            .iter()
            .filter(|i| i.path.starts_with(prefix) && (i.kind == IssueKind::UnknownKey || i.kind.is_error()))
            .map(|i| i.to_string())
            .collect();
        assert!(unknown.is_empty(), "{prefix}: {unknown:#?}");
    }
}
