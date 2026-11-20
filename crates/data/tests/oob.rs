use std::path::Path;

use common::Fixed;
use data::equipment::{self, Equipment};
use data::oob;
use data::provinces::ProvinceId;
use data::report::{IssueKind, Report};
use data::source::Source;

fn fixed(text: &str) -> Fixed {
    Fixed::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

const OOB: &str = r#"
start_equipment_factor = 0.2
division_template = {
	name = "Infanterie-Division"
	division_names_group = GER_INF_01
	regiments = {
		infantry = { x = 0 y = 0 }
		infantry = { x = 0 y = 1 }
		infantry = { x = 1 y = 0 }
	}
	support = {
		engineer = { x = 0 y = 0 }
		artillery = { x = 0 y = 1 }
	}
	regimental_support = { field_guns = { x = 1 y = 0 } }
	priority = 2
	is_locked = yes
}
division_template = { name = "Infanterie-Division" regiments = { } }
division_template = { regiments = { } }
units = {
	division = {
		division_name = { is_name_ordered = yes name_order = 7 }
		location = 6488
		division_template = "Infanterie-Division"
		start_experience_factor = 0.3
		start_equipment_factor = 0.75
		force_equipment_variants = { infantry_equipment_0 = { owner = "GER" version_name = "Gewehr 98" } }
	}
	division = {
		name = "Leibstandarte"
		location = 241
		division_template = "Infanterie-Division"
		officer = { name = "Someone" }
	}
	division = { location = 1 }
	fleet = { name = "Flotte" naval_base = 241 }
}
air_wings = { 64 = { fighter_equipment_0 = { owner = "GER" amount = 24 } } }
instant_effect = { add_equipment_production = { } }
"#;

#[test]
fn templates_and_divisions() {
    let mut report = Report::default();
    let o = oob::parse_file(&Source::from_text("history/units/GER_1936.txt", OOB.to_string()), &mut report);
    assert_eq!(o.name, "GER_1936");
    assert_eq!(o.start_equipment_factor, Some(fixed("0.2")));
    assert_eq!(o.templates.len(), 1, "the redefinition and the nameless template are dropped");
    let t = &o.templates[0];
    assert_eq!((t.regiments.len(), t.support.len(), t.regimental_support.len()), (3, 2, 1));
    assert_eq!((t.regiments[1].sub_unit.as_str(), t.regiments[1].x, t.regiments[1].y), ("infantry", 0, 1));
    assert_eq!((t.division_names_group.as_deref(), t.priority), (Some("GER_INF_01"), Some(2)));
    assert_eq!(t.flags, vec![("is_locked".to_string(), true)]);
    assert_eq!(o.divisions.len(), 3);
    let first = &o.divisions[0];
    assert_eq!(
        (first.name_order, first.location, first.template.as_str()),
        (Some(7), Some(ProvinceId(6488)), "Infanterie-Division")
    );
    assert_eq!(
        (first.start_experience_factor, first.start_equipment_factor),
        (Some(fixed("0.3")), Some(fixed("0.75")))
    );
    let forced = &first.force_equipment_variants[0];
    assert_eq!(
        (forced.equipment.as_str(), forced.owner.as_deref(), forced.version_name.as_deref()),
        ("infantry_equipment_0", Some("GER"), Some("Gewehr 98"))
    );
    assert!(o.divisions[1].has_officer && o.divisions[1].name.as_deref() == Some("Leibstandarte"));
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);
    assert_eq!(report.count(&IssueKind::Missing("name")), 1);
    assert_eq!(report.count(&IssueKind::Missing("division_template")), 1);
    // officer, fleet, air_wings, instant_effect
    assert_eq!(report.count(&IssueKind::Deferred), 4);
    assert_eq!(report.count(&IssueKind::UnknownKey), 0);
}

const EQUIPMENT: &str = r#"
equipments = {
	infantry_equipment = {
		is_archetype = yes
		is_buildable = no
		type = infantry
		group_by = archetype
		interface_category = interface_category_land
		reliability = 0.9
		soft_attack = 3
		build_cost_ic = 0.43
		resources = { steel = 1 }
	}
	infantry_equipment_1 = {
		archetype = infantry_equipment
		parent = infantry_equipment_0
		soft_attack = 4
	}
	infantry_equipment_0 = { archetype = infantry_equipment }
	loop_a = { archetype = loop_b }
	loop_b = { archetype = loop_a }
	orphan = { archetype = nowhere }
}
duplicate_archetypes = {
	infantry_copy = { archetype = infantry_equipment type = support }
}
"#;

#[test]
fn equipment_inherits_from_its_archetype() {
    let mut report = Report::default();
    let mut e = Equipment::default();
    equipment::parse_file(
        &Source::from_text("common/units/equipment/x.txt", EQUIPMENT.to_string()),
        &mut report,
        &mut e,
    );
    let v = e.effective("infantry_equipment_1").expect("variant");
    assert_eq!(v.number("soft_attack"), Some(fixed("4")), "own value wins");
    assert_eq!(v.number("reliability"), Some(fixed("0.9")), "inherited");
    assert_eq!(v.number("build_cost_ic"), Some(fixed("0.43")));
    assert_eq!(v.types, vec!["infantry"]);
    assert_eq!(v.resources, vec![("steel".to_string(), fixed("1"))]);
    assert_eq!(v.text("interface_category"), Some("interface_category_land"));
    assert_eq!(v.text("parent"), Some("infantry_equipment_0"), "parent is not followed for stats");
    assert!(!v.is_archetype(), "is_archetype is not inherited");
    assert_eq!(v.flag("is_buildable"), None, "is_buildable is not inherited");
    // A duplicate archetype copies the stats but not the type.
    let copy = e.effective("infantry_copy").expect("copy");
    assert_eq!(copy.types, vec!["support"]);
    assert_eq!(copy.number("soft_attack"), Some(fixed("3")));
    assert!(copy.is_archetype());
    // Broken chains: no answer instead of a guess or a hang.
    assert!(e.effective("loop_a").is_none());
    assert!(e.effective("orphan").is_none());
    assert!(e.effective("missing").is_none());
    assert_eq!(e.effective("infantry_equipment").map(|a| a.number("soft_attack")), Some(Some(fixed("3"))));
}

/// The real installation (skipped without `HOI4_DIR`).
#[test]
fn real_oobs_and_effective_equipment() {
    let Ok(dir) = std::env::var("HOI4_DIR") else { return };
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let mut report = Report::default();
    let oobs = oob::load(&vfs, &mut report);
    assert!(oobs.len() > 500);
    let ger = oobs.get("GER_1936").expect("GER_1936");
    assert!(!ger.templates.is_empty() && !ger.divisions.is_empty());
    assert!(ger.templates.iter().all(|t| !t.regiments.is_empty()), "every template has regiments");
    assert!(ger.divisions.iter().all(|d| d.location.is_some() && !d.template.is_empty()));
    assert_eq!(
        report.count(&IssueKind::UnknownKey),
        0,
        "{:?}",
        report.issues.iter().find(|i| i.kind == IssueKind::UnknownKey)
    );

    let mut report = Report::default();
    let eq = equipment::load(&vfs, &mut report);
    let rifle = eq.effective("infantry_equipment_1").expect("infantry_equipment_1");
    assert!(!rifle.types.is_empty(), "type comes from the archetype");
    assert!(rifle.number("soft_attack").is_some() && rifle.number("build_cost_ic").is_some());
    // Every non-archetype equipment resolves through its chain.
    let broken: Vec<&String> = eq.by_name.keys().filter(|n| eq.effective(n).is_none()).collect();
    assert!(broken.is_empty(), "{broken:?}");
}
