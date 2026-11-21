use std::path::Path;

use common::Fixed;
use data::equipment::{self, Equipment};
use data::report::{IssueKind, Report};
use data::source::Source;
use data::technologies::{self, Technologies};
use data::units::{self, Units};
use data::{Database, LoadOptions};

fn fixed(text: &str) -> Fixed {
    Fixed::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn pair(k: &str, v: &str) -> (String, Fixed) {
    (k.to_string(), fixed(v))
}

fn src(path: &str, text: &str) -> Source {
    Source::from_text(path, text.to_string())
}

const TAGS: &str = r#"
technology_categories = {
	infantry_weapons
	industry
	infantry_weapons
}
technology_folders = {
	infantry_folder = { ledger = army }
	armour_folder = {
		ledger = army
		available = { NOT = { has_dlc = "No Step Back" } }
	}
	land_doctrine_folder = { ledger = army doctrine = yes }
	infantry_folder = { ledger = navy }
}
"#;

const TECHS: &str = r#"
technologies = {
	@1936 = 2
	@x = -1

	infantry_weapons = {
		enable_equipments = { infantry_equipment_0 }
		enable_equipments = {
			limit = { NOT = { has_dlc = "By Blood Alone" } }
			rocket_equipment_1
		}
		enable_equipment_modules = { tank_heavy_machine_gun }
		enable_subunits = { infantry }
		path = { leads_to_tech = infantry_weapons1 research_cost_coeff = 1 }
		path = { leads_to_tech = support_weapons research_cost_coeff = 0.5 ignore_for_layout = yes }
		path = { }
		path = { research_cost_coeff = 1 }
		research_cost = 1.5
		start_year = 1918
		folder = { name = infantry_folder position = { x = @x y = @1936 } }
		categories = { infantry_weapons }
		ai_will_do = { factor = 1 }
		allow = { always = no }
	}
	infantry_weapons1 = {
		research_speed_factor = 0.03
		category_light_infantry = {
			soft_attack = 0.05
			forest = { attack = 0.05 movement = 0.1 }
			battalion_mult = { category = category_light_infantry add = yes display_as_percentage = yes max_organisation = 2 }
		}
		infantry = { hard_attack = 0.1 }
		XOR = { infantry_weapons }
		xor = { other }
		dependencies = { infantry_weapons = 1 }
		sub_technologies = { cv_fighter }
		sub_tech_index = 1
		enable_building = { building = radar_station level = 1 }
		enable_building = { building = radar_station }
		special_project_specialization = { specialization_land specialization_air }
		ai_research_weights = { offensive = -1.0 }
		is_special_project_tech = yes
		xp_research_type = navy
		xp_boost_cost = 50
		desc = "SPECIAL_DESC"
		mystery = yes
		folder = { name = infantry_folder position = { x = 1 } }
		on_research_complete = { custom_effect_tooltip = X }
	}
	infantry_weapons = { research_cost = 9 }
}
"#;

fn load(tags: &str, techs: &str) -> (Technologies, Report) {
    let mut report = Report::default();
    let mut out = Technologies::default();
    technologies::parse_tags(&src("common/technology_tags/00_technology.txt", tags), &mut report, &mut out);
    technologies::parse_file(&src("common/technologies/infantry.txt", techs), &mut report, &mut out);
    (out, report)
}

#[test]
fn tags_keep_categories_and_folders() {
    let (t, report) = load(TAGS, "");
    assert_eq!(t.categories, ["infantry_weapons", "industry"]);
    let names: Vec<&str> = t.folders.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["infantry_folder", "armour_folder", "land_doctrine_folder"]);
    assert_eq!(t.folder("infantry_folder").and_then(|f| f.ledger.as_deref()), Some("army"), "first kept");
    assert!(t.folder("armour_folder").is_some_and(|f| f.conditional && !f.doctrine));
    assert!(t.folder("land_doctrine_folder").is_some_and(|f| f.doctrine && !f.conditional));
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 2, "repeated category and folder");
    assert_eq!(report.count(&IssueKind::Deferred), 1, "the available trigger");
}

#[test]
fn technologies_keep_unlocks_paths_and_bonuses() {
    let (t, report) = load(TAGS, TECHS);
    assert_eq!(t.len(), 2);
    let w = t.get("infantry_weapons").expect("infantry_weapons");
    assert_eq!(w.research_cost, Some(fixed("1.5")), "the first definition is kept");
    assert_eq!(w.start_year, Some(1918));
    assert_eq!(w.folders.len(), 1);
    assert_eq!(
        (w.folders[0].folder.as_str(), w.folders[0].x, w.folders[0].y),
        ("infantry_folder", fixed("-1"), fixed("2"))
    );
    assert_eq!(w.categories, ["infantry_weapons"]);
    assert_eq!(w.paths.len(), 2, "the empty path says nothing; the one without a target is reported");
    assert_eq!(w.paths[0].leads_to, "infantry_weapons1");
    assert!(!w.paths[0].ignore_for_layout);
    assert_eq!(w.paths[1].research_cost_coeff, Some(fixed("0.5")));
    assert!(w.paths[1].ignore_for_layout);
    assert_eq!(w.enable_equipments.len(), 2);
    assert!(!w.enable_equipments[0].conditional);
    assert!(w.enable_equipments[1].conditional);
    assert_eq!(w.enabled_equipment().collect::<Vec<_>>(), ["infantry_equipment_0", "rocket_equipment_1"]);
    assert_eq!(w.enable_equipment_modules[0].names, ["tank_heavy_machine_gun"]);
    assert_eq!(w.enable_subunits[0].names, ["infantry"]);
    assert!(w.modifiers.is_empty() && w.unit_stats.is_empty());

    let w1 = t.get("infantry_weapons1").expect("infantry_weapons1");
    assert_eq!(w1.modifier("research_speed_factor"), Some(fixed("0.03")));
    let light = w1.unit_stats("category_light_infantry").expect("category stats");
    assert_eq!(light.stats, vec![pair("soft_attack", "0.05")]);
    assert_eq!(light.terrain.get("forest"), Some(&vec![pair("attack", "0.05"), pair("movement", "0.1")]));
    assert_eq!(light.battalion_mult.len(), 1);
    let mult = &light.battalion_mult[0];
    assert!(mult.add && mult.display_as_percentage);
    assert_eq!(mult.values, vec![pair("max_organisation", "2")]);
    assert_eq!(w1.unit_stats("infantry").map(|u| u.stats.clone()), Some(vec![pair("hard_attack", "0.1")]));
    assert_eq!(w1.xor, ["infantry_weapons", "other"], "both spellings");
    assert_eq!(w1.dependencies, vec![pair("infantry_weapons", "1")]);
    assert_eq!(w1.sub_technologies, ["cv_fighter"]);
    assert_eq!(w1.number("sub_tech_index"), Some(fixed("1")));
    assert_eq!(w1.number("xp_boost_cost"), Some(fixed("50")));
    assert_eq!(w1.enable_buildings, vec![("radar_station".to_string(), 1)]);
    assert_eq!(w1.special_project_specialization, ["specialization_land", "specialization_air"]);
    assert_eq!(w1.ai_research_weights, vec![pair("offensive", "-1.0")]);
    assert_eq!(w1.flag("is_special_project_tech"), Some(true));
    assert_eq!(w1.text("xp_research_type"), Some("navy"));
    assert_eq!(w1.text("desc"), Some("SPECIAL_DESC"));
    assert!(w1.folders.is_empty(), "position without y");

    assert_eq!(report.count(&IssueKind::Missing("leads_to_tech")), 1);
    assert_eq!(report.count(&IssueKind::Missing("building and level")), 1);
    assert_eq!(report.count(&IssueKind::Missing("x and y")), 1);
    assert_eq!(report.count(&IssueKind::Missing("name and position")), 1);
    assert_eq!(report.count(&IssueKind::UnknownKey), 1, "mystery = yes");
    // ai_will_do, allow, the enable_equipments limit, on_research_complete; plus the folder's `available`.
    assert_eq!(report.count(&IssueKind::Deferred), 5);
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 3, "category, folder, technology");
}

#[test]
fn constants_are_local_to_their_file() {
    let mut report = Report::default();
    let mut out = Technologies::default();
    technologies::parse_file(
        &src("a.txt", "technologies = { @y = 3 a = { folder = { name = f position = { x = 0 y = @y } } } }"),
        &mut report,
        &mut out,
    );
    technologies::parse_file(
        &src("b.txt", "technologies = { b = { folder = { name = f position = { x = 0 y = @y } } } }"),
        &mut report,
        &mut out,
    );
    assert_eq!(out.get("a").map(|t| t.folders[0].y), Some(fixed("3")));
    assert!(out.get("b").is_some_and(|t| t.folders.is_empty()));
    assert_eq!(report.count(&IssueKind::BrokenReference("constant")), 1);
}

const EQUIPMENT: &str = r#"
equipments = {
	light_tank_chassis = { is_archetype = yes }
	light_tank_chassis_1 = { archetype = light_tank_chassis }
	other_chassis = { is_archetype = yes }
	other_chassis_1 = { archetype = other_chassis }
}
duplicate_archetypes = {
	light_tank_aa_chassis = { archetype = light_tank_chassis }
}
"#;

fn load_equipment() -> Equipment {
    let mut report = Report::default();
    let mut out = Equipment::default();
    equipment::parse_file(&src("common/units/equipment/tank_chassis.txt", EQUIPMENT), &mut report, &mut out);
    out
}

#[test]
fn duplicate_archetypes_imply_copies_of_their_variants() {
    let e = load_equipment();
    let (dup, source) = e.duplicate_source("light_tank_aa_chassis_1").expect("implied copy");
    assert_eq!((dup.name.as_str(), source.name.as_str()), ("light_tank_aa_chassis", "light_tank_chassis_1"));
    assert!(e.exists("light_tank_aa_chassis_1"));
    assert!(e.exists("light_tank_chassis_1"));
    assert!(!e.exists("light_tank_aa_chassis_2"), "no source variant");
    assert!(e.duplicate_source("light_tank_aa_chassis").is_none(), "the duplicate itself is defined");
    assert!(e.duplicate_source("light_tank_aa_chassis_").is_none(), "source would be the archetype's name + `_`");
}

#[test]
fn technology_references_are_checked() {
    let mut report = Report::default();
    let mut db = Database { equipment: load_equipment(), ..Default::default() };
    let mut u = Units::default();
    units::parse_categories(
        &src("common/unit_tags/00.txt", "sub_unit_categories = { category_light_infantry }"),
        &mut report,
        &mut u,
    );
    units::parse_sub_units(&src("common/units/infantry.txt", "sub_units = { infantry = { } }"), &mut report, &mut u);
    db.units = u;
    let text = r#"
technologies = {
	a = {
		categories = { industry nope }
		folder = { name = infantry_folder position = { x = 0 y = 0 } }
		folder = { name = no_folder position = { x = 0 y = 0 } }
		path = { leads_to_tech = b }
		path = { leads_to_tech = gone }
		XOR = { gone }
		dependencies = { gone = 1 }
		sub_technologies = { gone }
		enable_equipments = { light_tank_aa_chassis_1 light_tank_chassis_1 ghost_equipment }
		enable_equipment_modules = { ghost_module }
		enable_subunits = { infantry ghost_unit }
		enable_building = { building = ghost_building level = 1 }
		infantry = { forest = { attack = 1 } fort = { attack = 1 } lava = { attack = 1 } }
		category_light_infantry = { battalion_mult = { category = category_ghost soft_attack = 1 } }
		ghost_unit = { soft_attack = 1 }
	}
	b = { }
}
"#;
    let (techs, _) = load(TAGS, text);
    db.technologies = techs;
    let mut refs = Report::default();
    db.check_references(&mut refs);
    let count = |what| refs.count(&IssueKind::BrokenReference(what));
    assert_eq!(count("technology category"), 1);
    assert_eq!(count("technology folder"), 1);
    assert_eq!(count("technology"), 4, "path, XOR, dependency, sub-technology");
    assert_eq!(count("equipment"), 1, "the implied copy and the defined variant resolve");
    assert_eq!(count("module"), 1);
    assert_eq!(count("sub-unit"), 1);
    assert_eq!(count("building"), 1);
    assert_eq!(count("sub-unit or sub-unit category"), 1);
    // No terrain is loaded here: `forest` and `lava` fail, the combat situation `fort` resolves.
    assert_eq!(count("terrain"), 2);
    assert_eq!(count("sub-unit category"), 1);
}

/// The real installation (skipped without `HOI4_DIR`).
#[test]
fn real_technologies() {
    let Ok(dir) = std::env::var("HOI4_DIR") else { return };
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let (db, report) = Database::load(&vfs, &LoadOptions::default());
    let t = &db.technologies;
    assert!(t.len() > 500, "{}", t.len());
    assert!(t.categories.len() > 100);
    assert!(t.folder("infantry_folder").is_some_and(|f| f.ledger.as_deref() == Some("army")));
    let w = t.get("infantry_weapons").expect("infantry_weapons");
    assert_eq!(w.enabled_equipment().collect::<Vec<_>>(), ["infantry_equipment_0"]);
    assert!(w.paths.iter().any(|p| p.leads_to == "infantry_weapons1"));
    let technology_issues =
        |kind: &IssueKind| report.issues.iter().filter(|i| i.path.starts_with("technolog") && &i.kind == kind).count();
    assert_eq!(technology_issues(&IssueKind::UnknownKey), 0);
    assert_eq!(technology_issues(&IssueKind::Duplicate), 0);
    // Defects of the original: five `path = { research_cost_coeff = 1 }` without a target (excavation1-5), the
    // category `air_equipment` declared twice, and `advanced_modern_tank_turret` enabled but defined nowhere.
    assert_eq!(technology_issues(&IssueKind::Missing("leads_to_tech")), 5);
    assert_eq!(technology_issues(&IssueKind::Redefinition("first")), 1);
    let broken: Vec<_> = report
        .issues
        .iter()
        .filter(|i| i.path.starts_with("technology") && matches!(i.kind, IssueKind::BrokenReference(_)))
        .collect();
    assert_eq!(broken.len(), 1, "{broken:#?}");
    assert_eq!(broken[0].path, "technology.enable_equipment_modules");
}
