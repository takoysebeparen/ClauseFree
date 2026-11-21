use std::path::Path;

use common::Fixed;
use data::buildings::{self, Buildings, SpawnKind};
use data::report::{IssueKind, Report};
use data::source::Source;
use data::{Database, LoadOptions, Tag};

fn fixed(text: &str) -> Fixed {
    Fixed::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn load(text: &str) -> (Buildings, Report) {
    let mut report = Report::default();
    let mut out = Buildings::default();
    buildings::parse_file(&Source::from_text("common/buildings/test.txt", text.to_string()), &mut report, &mut out);
    (out, report)
}

const SAMPLE: &str = r#"
@repair = 0.15

buildings = {
	arms_factory = {
		show_on_map = 6
		base_cost = 7200
		military_production = 1
		disabled_in_dmz = yes
		level_cap = { shares_slots = yes state_max = 20 }
	}
	naval_base = {
		base_cost = 5000
		spawn_point = naval_base_spawn
		tags = { naval_buildings_tag }
		level_cap = { province_max = 10 }
	}
	refinery = {
		local_resources_rubber = 1
		repair_speed_factor = @repair
		country_modifiers = {
			enable_for_controllers = { ENG }
			modifiers = { fuel_gain_from_states = 2.0 }
		}
		state_modifiers = { local_building_slots_factor = 0.15 }
		building_prerequisite = { building_dependency = { building = naval_base level = 8 } }
		level_cap = { state_max = 3 exclusive_with = arms_factory }
		dlc_allowed = { has_dlc = "Some DLC" }
		mystery_key = 1
		damage_factor = @missing
	}
	arms_factory = { base_cost = 1 }
}

spawn_points = {
	naval_base_spawn = { type = province max = 1 only_costal = yes }
	odd_spawn = { type = region }
}
"#;

#[test]
fn building_definitions() {
    let (b, report) = load(SAMPLE);
    assert_eq!(b.len(), 3, "the second arms_factory is a redefinition");
    let names: Vec<&str> = b.list.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["arms_factory", "naval_base", "refinery"], "definition order is kept");

    let arms = b.get("arms_factory").expect("arms_factory");
    assert_eq!(arms.number("base_cost"), Some(fixed("7200")));
    assert_eq!(arms.flag("disabled_in_dmz"), Some(true));
    assert_eq!(arms.level_cap.state_max, Some(20));
    assert_eq!(arms.level_cap.shares_slots, Some(true));
    assert!(!arms.is_provincial());

    let naval = b.get("naval_base").expect("naval_base");
    assert!(naval.is_provincial());
    assert_eq!(naval.text("spawn_point"), Some("naval_base_spawn"));
    assert_eq!(naval.tags, ["naval_buildings_tag"]);

    let r = b.get("refinery").expect("refinery");
    assert_eq!(r.local_resources, vec![("rubber".to_string(), fixed("1"))]);
    assert_eq!(r.number("repair_speed_factor"), Some(fixed("0.15")), "resolved through @repair");
    assert_eq!(r.number("damage_factor"), None, "unknown constant");
    let cm = r.country_modifiers.as_ref().expect("country modifiers");
    assert_eq!(cm.enable_for_controllers, vec![Tag::parse("ENG").expect("tag")]);
    assert_eq!(cm.modifiers, vec![("fuel_gain_from_states".to_string(), fixed("2"))]);
    assert_eq!(r.state_modifiers, vec![("local_building_slots_factor".to_string(), fixed("0.15"))]);
    assert_eq!(r.prerequisites.len(), 1);
    assert_eq!((r.prerequisites[0].building.as_str(), r.prerequisites[0].level), ("naval_base", 8));
    assert_eq!(r.level_cap.exclusive_with, ["arms_factory"]);

    let spawn = &b.spawn_points["naval_base_spawn"];
    assert_eq!((spawn.kind, spawn.max, spawn.only_costal), (SpawnKind::Province, Some(1), Some(true)));
    assert!(!b.spawn_points.contains_key("odd_spawn"), "invalid type");

    let has = |path: &str, kind: IssueKind| report.issues.iter().any(|i| i.path == path && i.kind == kind);
    assert!(has("building", IssueKind::Redefinition("first")));
    assert!(has("building.mystery_key", IssueKind::UnknownKey));
    assert!(has("building.dlc_allowed", IssueKind::Deferred));
    assert!(has("building.damage_factor", IssueKind::BrokenReference("constant")));
    assert!(has("spawn_point.type", IssueKind::InvalidValue("state or province")));
}

#[test]
fn constants_are_local_to_their_file() {
    let mut report = Report::default();
    let mut out = Buildings::default();
    let first = "@cost = 10\nbuildings = { a = { base_cost = @cost } }";
    let second = "buildings = { b = { base_cost = @cost } }";
    for (i, text) in [first, second].into_iter().enumerate() {
        let src = Source::from_text(&format!("common/buildings/{i}.txt"), text.to_string());
        buildings::parse_file(&src, &mut report, &mut out);
    }
    assert_eq!(out.get("a").and_then(|b| b.number("base_cost")), Some(fixed("10")));
    assert_eq!(out.get("b").and_then(|b| b.number("base_cost")), None);
    assert_eq!(report.count(&IssueKind::BrokenReference("constant")), 1);
}

/// The real installation (skipped without `HOI4_DIR`).
#[test]
fn real_buildings() {
    let Ok(dir) = std::env::var("HOI4_DIR") else { return };
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let (db, report) = Database::load(&vfs, &LoadOptions::default());
    assert!(db.buildings.len() > 40);
    let infrastructure = db.buildings.get("infrastructure").expect("infrastructure");
    assert!(!infrastructure.is_provincial());
    assert!(db.buildings.get("naval_base").is_some_and(|b| b.is_provincial()));
    let building_issue = |i: &&data::Issue| {
        (i.path.starts_with("building") || i.path.starts_with("spawn_point") || i.path.contains("buildings"))
            && matches!(i.kind, IssueKind::UnknownKey | IssueKind::BrokenReference(_) | IssueKind::InvalidValue(_))
    };
    let bad: Vec<_> = report.issues.iter().filter(building_issue).collect();
    assert!(bad.is_empty(), "{bad:?}");
}
