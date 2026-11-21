use std::path::Path;

use common::Fixed;
use data::equipment::{self, CountRule, Equipment, LimitMatch, ModuleSlots, SlotSpec};
use data::modules::{self, ConvertFrom, Modules};
use data::report::{IssueKind, Report};
use data::source::Source;
use data::{Database, LoadOptions};

fn fixed(text: &str) -> Fixed {
    Fixed::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn pair(k: &str, v: &str) -> (String, Fixed) {
    (k.to_string(), fixed(v))
}

const MODULES: &str = r#"
equipment_modules = {
	tank_gasoline_engine = {
		abbreviation = "gas"
		category = tank_engine_type
		sfx = sfx_ui_sd_module_engine
		add_stats = { build_cost_ic = 1 maximum_speed = 0.5 }
		multiply_stats = { maximum_speed = 0.15 }
		multiply_stats = { }
		xp_cost = 1
	}
	tank_turret = {
		category = tank_turret_type
		forbid_equipment_type = anti_air
		forbid_equipment_type_exact_match = armor
		allowed_module_categories = { main_armament_slot = { tank_engine_type } }
		forbid_equipment_type_exact_match_for_category = { tank_engine_type = armor }
		build_cost_resources = { steel = 1 }
		can_convert_from = { module_category = tank_engine_type convert_cost_ic = 60 }
		can_convert_from = { module = tank_gasoline_engine convert_cost_ic = 45 convert_cost_resources = { steel = 0 } }
		can_convert_from = { }
		can_convert_from = { convert_cost_ic = 1 }
	}
}
equipment_modules = {
	limit = { has_dlc = "Some DLC" }
	plane_gun = {
		category = fighter_weapon
		parent = tank_turret
		add_average_stats = { lg_armor_piercing = 1 }
		allow_equipment_type = { fighter cas }
		critical_parts = { damaged_light_guns }
		mission_type_stats = {
			limit = { interception }
			multiply_stats = { air_agility = 0.1 }
		}
		mega_carrier = yes
		mystery = 1
	}
	tank_gasoline_engine = { category = tank_engine_type }
	no_category = { xp_cost = 2 }
}
"#;

fn load_modules(text: &str) -> (Modules, Report) {
    let mut report = Report::default();
    let mut out = Modules::default();
    modules::parse_file(
        &Source::from_text("common/units/equipment/modules/00_test.txt", text.to_string()),
        &mut report,
        &mut out,
    );
    (out, report)
}

#[test]
fn modules_keep_stats_conversions_and_restrictions() {
    let (m, report) = load_modules(MODULES);
    let names: Vec<&str> = m.list.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["tank_gasoline_engine", "tank_turret", "plane_gun", "no_category"], "definition order");
    let engine = m.get("tank_gasoline_engine").expect("engine");
    assert_eq!(engine.category, "tank_engine_type");
    assert_eq!(engine.text("abbreviation"), Some("gas"));
    assert_eq!(engine.stats.add, vec![pair("build_cost_ic", "1"), pair("maximum_speed", "0.5")]);
    assert_eq!(engine.stats.multiply, vec![pair("maximum_speed", "0.15")]);
    assert_eq!(engine.number("xp_cost"), Some(fixed("1")));
    assert!(!engine.conditional);

    let turret = m.get("tank_turret").expect("turret");
    assert_eq!(turret.list("forbid_equipment_type"), ["anti_air"]);
    assert_eq!(turret.list("forbid_equipment_type_exact_match"), ["armor"]);
    assert_eq!(
        turret.allowed_module_categories,
        vec![("main_armament_slot".to_string(), vec!["tank_engine_type".to_string()])]
    );
    assert_eq!(turret.forbid_exact_match_for_category, vec![("tank_engine_type".to_string(), "armor".to_string())]);
    assert_eq!(turret.build_cost_resources, vec![pair("steel", "1")]);
    assert_eq!(turret.conversions.len(), 2, "the empty block says nothing; the one without a source is reported");
    assert_eq!(turret.conversions[0].from, ConvertFrom::Category("tank_engine_type".to_string()));
    assert_eq!(turret.conversions[1].from, ConvertFrom::Module("tank_gasoline_engine".to_string()));
    assert_eq!(turret.conversions[1].cost_ic, Some(fixed("45")));
    assert_eq!(turret.conversions[1].cost_resources, vec![pair("steel", "0")]);

    let gun = m.get("plane_gun").expect("gun");
    assert!(gun.conditional, "its block has a limit");
    assert_eq!(gun.stats.average, vec![pair("lg_armor_piercing", "1")]);
    assert_eq!(gun.list("allow_equipment_type"), ["fighter", "cas"]);
    assert_eq!(gun.list("critical_parts"), ["damaged_light_guns"]);
    assert_eq!(gun.mission_stats.len(), 1);
    assert_eq!(gun.mission_stats[0].missions, ["interception"]);
    assert_eq!(gun.mission_stats[0].stats.multiply, vec![pair("air_agility", "0.1")]);
    assert_eq!(gun.flag("mega_carrier"), Some(true));

    assert_eq!(
        m.categories().into_iter().collect::<Vec<_>>(),
        ["fighter_weapon", "tank_engine_type", "tank_turret_type"]
    );
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1, "second tank_gasoline_engine");
    assert_eq!(report.count(&IssueKind::Missing("category")), 1);
    assert_eq!(report.count(&IssueKind::InvalidValue("exactly one of module and module_category")), 1);
    assert_eq!(report.count(&IssueKind::UnknownKey), 1, "mystery");
    assert_eq!(report.count(&IssueKind::Deferred), 1, "the limit trigger");
}

const MODULAR_EQUIPMENT: &str = r#"
equipments = {
	hull = {
		is_archetype = yes
		module_slots = {
			fixed_engine_slot = { required = yes allowed_module_categories = { tank_engine_type } }
			custom_1 = { required = no allowed_module_categories = { tank_engine_type tank_turret_type } gfx = deck }
			custom_2 = custom_1
		}
		module_count_limit = { category = tank_turret_type count < 2 }
		module_count_limit = { module = tank_gasoline_engine module = empty count > 0 }
		default_modules = { fixed_engine_slot = tank_gasoline_engine }
		module_add_stats_multiplier = { soft_attack = -0.05 }
	}
	hull_1 = {
		archetype = hull
		module_slots = inherit
		module_count_limit = { category = tank_turret_type count = any }
	}
	hull_2 = {
		archetype = hull
		parent = hull_big
		module_slots = { fixed_engine_slot = inherit extra = inherit }
		default_modules = { fixed_engine_slot = inherit extra = empty }
	}
	hull_big = {
		archetype = hull
		module_slots = {
			fixed_engine_slot = { required = no allowed_module_categories = { tank_turret_type } }
			extra = { allowed_module_categories = { tank_turret_type } }
		}
		default_modules = { fixed_engine_slot = tank_turret }
	}
	hull_none = { archetype = hull module_slots = none }
	hull_absent = { archetype = hull }
	hull_bad = { archetype = hull module_slots = { nowhere = inherit } }
	odd = {
		module_slots = { a = b }
		module_count_limit = { count < 1 }
		module_count_limit = { module = x count >= 1 }
		module_slots = inherit
	}
}
"#;

fn load_equipment(text: &str) -> (Equipment, Report) {
    let mut report = Report::default();
    let mut out = Equipment::default();
    equipment::parse_file(
        &Source::from_text("common/units/equipment/hull.txt", text.to_string()),
        &mut report,
        &mut out,
    );
    (out, report)
}

fn slot_names(slots: &Option<ModuleSlots>) -> Vec<&str> {
    match slots {
        Some(ModuleSlots::List(l)) => l.iter().map(|s| s.name.as_str()).collect(),
        _ => Vec::new(),
    }
}

#[test]
fn modular_equipment_slots_limits_and_defaults() {
    let (e, report) = load_equipment(MODULAR_EQUIPMENT);
    let hull = e.get("hull").expect("hull");
    let Some(ModuleSlots::List(slots)) = &hull.module_slots else { panic!("slots") };
    assert_eq!(slot_names(&hull.module_slots), ["fixed_engine_slot", "custom_1", "custom_2"]);
    assert!(slots[0].is_fixed() && !slots[1].is_fixed());
    assert_eq!(
        slots[1].spec,
        SlotSpec::Defined {
            required: Some(false),
            allowed_categories: vec!["tank_engine_type".to_string(), "tank_turret_type".to_string()],
            gfx: Some("deck".to_string()),
        }
    );
    assert_eq!(slots[2].spec, SlotSpec::SameAs("custom_1".to_string()));
    assert_eq!(hull.module_count_limits.len(), 2);
    assert_eq!(hull.module_count_limits[0].count, CountRule::Below(2));
    assert_eq!(
        hull.module_count_limits[1].matches,
        vec![LimitMatch::Module("tank_gasoline_engine".to_string()), LimitMatch::Module("empty".to_string())]
    );
    assert_eq!(hull.module_count_limits[1].count, CountRule::Above(0));
    assert_eq!(hull.module_add_stats_multiplier, vec![pair("soft_attack", "-0.05")]);

    // `module_slots = inherit` without `parent`: the archetype's slots; `<slot> = <earlier slot>` is copied.
    let h1 = e.effective("hull_1").expect("hull_1");
    let Some(ModuleSlots::List(s1)) = &h1.module_slots else { panic!("slots") };
    assert_eq!(slot_names(&h1.module_slots), ["fixed_engine_slot", "custom_1", "custom_2"]);
    assert_eq!(s1[2].spec, s1[1].spec);
    // A limit with the same match conditions overrides the inherited one; the other is inherited.
    assert_eq!(h1.module_count_limits.len(), 2);
    assert_eq!(h1.module_count_limits[0].count, CountRule::Any);
    assert_eq!(h1.module_count_limits[1].count, CountRule::Above(0));
    assert_eq!(h1.default_modules, vec![("fixed_engine_slot".to_string(), "tank_gasoline_engine".to_string())]);

    // `inherit` with a `parent` takes the parent's slot and default module, not the archetype's.
    let h2 = e.effective("hull_2").expect("hull_2");
    let Some(ModuleSlots::List(s2)) = &h2.module_slots else { panic!("slots") };
    assert_eq!(slot_names(&h2.module_slots), ["fixed_engine_slot", "extra"]);
    assert!(matches!(&s2[0].spec, SlotSpec::Defined { required: Some(false), .. }));
    assert_eq!(h2.default_modules[0], ("fixed_engine_slot".to_string(), "tank_turret".to_string()));

    assert_eq!(e.effective("hull_none").and_then(|h| h.module_slots), Some(ModuleSlots::None));
    assert_eq!(slot_names(&e.effective("hull_absent").expect("absent").module_slots).len(), 3, "absent: archetype's");
    assert!(e.effective("hull_bad").is_none(), "a slot that cannot be inherited");

    assert_eq!(report.count(&IssueKind::BrokenReference("earlier slot")), 1, "a = b");
    assert_eq!(report.count(&IssueKind::Missing("module or category")), 1);
    assert_eq!(report.count(&IssueKind::InvalidValue("count < N, count > N, count = N or count = any")), 1);
    assert_eq!(report.count(&IssueKind::Duplicate), 1, "second module_slots");
    assert_eq!(report.count(&IssueKind::UnknownKey), 0);
}

#[test]
fn module_references_are_checked() {
    let (modules, _) = load_modules(
        r#"equipment_modules = {
            engine = { category = engine_type build_cost_resources = { steel = 1 gold = 1 } }
            turret = {
                category = turret_type
                parent = ghost
                can_convert_from = { module = phantom }
                can_convert_from = { module_category = no_such_category }
                forbid_module_categories = { engine_type nothing_here }
            }
        }"#,
    );
    let (equipment, _) = load_equipment(
        r#"equipments = {
            hull = {
                is_archetype = yes
                module_slots = { fixed_a = { allowed_module_categories = { engine_type imaginary } } }
                module_count_limit = { module = any category = any count < 3 }
                module_count_limit = { module = nonexistent category = turret_type count < 1 }
                default_modules = { fixed_a = engine fixed_b = empty fixed_a = missing_module }
            }
        }"#,
    );
    let mut resources = data::resources::Resources::default();
    resources.list.push(data::resources::ResourceDef { name: "steel".to_string(), ..Default::default() });
    let db = Database { modules, equipment, resources, ..Default::default() };
    let mut report = Report::default();
    db.check_references(&mut report);
    let broken = |what: &'static str| report.count(&IssueKind::BrokenReference(what));
    assert_eq!(broken("module"), 4, "parent ghost, conversion phantom, limit nonexistent, default missing_module");
    assert_eq!(broken("module category"), 3, "no_such_category, nothing_here, imaginary");
    assert_eq!(broken("resource"), 1, "gold");
    assert_eq!(broken("slot of this equipment"), 1, "fixed_b");
}

/// The real installation (skipped without `HOI4_DIR`).
#[test]
fn real_modules_and_modular_equipment() {
    let Ok(dir) = std::env::var("HOI4_DIR") else { return };
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let (db, report) = Database::load(&vfs, &LoadOptions::default());
    assert!(db.modules.len() > 300, "{}", db.modules.len());
    assert!(db.modules.categories().len() > 50);
    assert!(db.modules.get("tank_gasoline_engine").is_some_and(|m| m.category == "tank_engine_type"));
    // `heavy_cruiser_3` inherits its slots through `parent` (heavy_cruiser_2 -> heavy_cruiser_1).
    let hc3 = db.equipment.effective("heavy_cruiser_3").expect("heavy_cruiser_3");
    assert!(slot_names(&hc3.module_slots).contains(&"rear_2_custom_slot"));
    let modular: Vec<&String> = db
        .equipment
        .by_name
        .iter()
        .filter(|(_, e)| e.module_slots.is_some())
        .map(|(n, _)| n)
        .filter(|n| db.equipment.effective(n).is_none())
        .collect();
    assert!(modular.is_empty(), "{modular:?}");
    let unknown: Vec<String> = report
        .issues
        .iter()
        .filter(|i| i.kind == IssueKind::UnknownKey && i.file.contains("units/equipment"))
        .map(|i| i.to_string())
        .collect();
    assert!(unknown.is_empty(), "{unknown:?}");
}
