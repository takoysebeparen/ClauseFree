use std::path::{Path, PathBuf};

use common::Fixed;
use data::countries::{self, Countries, CountryDef};
use data::equipment::{self, Equipment};
use data::localisation::{self, Localisation};
use data::provinces::{self, ProvinceId, ProvinceKind};
use data::resources::{self, Resources};
use data::source::{Source, evaluation_order};
use data::states::{self, StateId, States};
use data::terrain::{self, Terrain};
use data::units::{self, Units};
use data::{Color, Database, IssueKind, LoadOptions, Report, Tag};

fn src(path: &str, text: &str) -> Source {
    Source::from_text(path, text.to_string())
}

fn fixed(text: &str) -> Fixed {
    Fixed::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn tag(text: &str) -> Tag {
    Tag::parse(text).unwrap_or_else(|| panic!("bad tag {text}"))
}

/// Scratch game directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("clausefree_data_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, content).expect("write");
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn province_definitions() {
    let mut report = Report::default();
    let text = "0;0;0;0;land;false;unknown;0\n1;230;81;119;lake;false;lakes;7\n2;0;0;55;land;true;forest;1\n\
                3;0;0;55;sea;false;ocean;0\n2;1;1;1;land;false;plains;1\nbroken line\n4;1;2;3;swamp;false;x;1\n";
    let p = provinces::parse_definitions(&src("map/definition.csv", text), &mut report);
    assert_eq!(p.len(), 4, "0, 1, 2 and 3; the second 2 is a redefinition");
    let two = p.get(ProvinceId(2)).expect("province 2");
    assert_eq!(
        (two.color, two.kind, two.coastal, two.terrain.as_str(), two.continent),
        ([0, 0, 55], ProvinceKind::Land, true, "forest", 1)
    );
    assert_eq!(p.get(ProvinceId(1)).map(|d| d.kind), Some(ProvinceKind::Lake));
    assert_eq!(p.by_color.get(&[0, 0, 55]), Some(&ProvinceId(2)), "colour clash keeps the first");
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 2, "duplicate id and duplicate colour");
    assert_eq!(report.issues.iter().filter(|i| matches!(i.kind, IssueKind::InvalidValue(_))).count(), 2);
    assert!(report.issues.iter().all(|i| i.line > 0));
}

#[test]
fn terrain_categories_with_modifier_blocks() {
    let mut report = Report::default();
    let mut t = Terrain::default();
    let text = "categories = {\n forest = {\n  color = { 89 199 85 }\n  movement_cost = 1.5\n  combat_width = 60\n  \
                buildings_max_level = { bunker = 4 }\n  units = { attack = -0.15 }\n  submarine = { navy_visibility = -0.15 units = { movement = -0.25 } }\n  \
                mystery = 1\n }\n ocean = { is_water = yes naval_terrain = yes }\n}\nterrain = { x = { type = forest } }\n";
    terrain::parse_file(&src("common/terrain/00_terrain.txt", text), &mut report, &mut t);
    let forest = t.get("forest").expect("forest");
    assert_eq!(forest.color, Some(Color::Rgb([89, 199, 85])));
    assert_eq!((forest.movement_cost, forest.combat_width), (Some(fixed("1.5")), Some(fixed("60"))));
    assert_eq!(forest.buildings_max_level, vec![("bunker".to_string(), fixed("4"))]);
    assert_eq!(forest.unit_modifiers, vec![("attack".to_string(), fixed("-0.15"))]);
    let sub = forest.unit_type_modifiers.get("submarine").expect("submarine");
    assert_eq!(sub.values, vec![("navy_visibility".to_string(), fixed("-0.15"))]);
    assert_eq!(sub.units, vec![("movement".to_string(), fixed("-0.25"))]);
    assert!(t.get("ocean").is_some_and(|o| o.is_water && o.naval_terrain));
    // `mystery = 1` is a plain value where only blocks are allowed: reported, not kept.
    assert!(report.issues.iter().any(|i| i.path.ends_with("mystery")));
    assert_eq!(report.count(&IssueKind::Deferred), 1, "graphical terrain goes to the renderer");
}

#[test]
fn country_tags_files_and_colors() {
    let mut report = Report::default();
    let mut c = Countries::default();
    countries::parse_tags(
        &src(
            "common/country_tags/00_countries.txt",
            "GER = \"countries/Germany.txt\"\nENG = \"countries/UK.txt\"\nGER = \"countries/Other.txt\"\nbad = \"x\"\ndynamic_tags = yes\nD01 = \"countries/D01.txt\"\n",
        ),
        &mut report,
        &mut c,
    );
    assert_eq!(c.order, vec![tag("GER"), tag("ENG"), tag("D01")], "definition order is kept");
    assert_eq!(c.get(tag("GER")).map(|d| d.file.as_str()), Some("countries/Germany.txt"), "first definition kept");
    assert!(c.get(tag("D01")).is_some_and(|d| d.dynamic) && !c.get(tag("GER")).is_some_and(|d| d.dynamic));
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);

    let mut ger = CountryDef::default();
    countries::parse_country_file(
        &src(
            "common/countries/Germany.txt",
            "graphical_culture = western_european_gfx\ncolor = { 106 119 89 }\nuse_legacy_ai_pp_spend = yes\n",
        ),
        &mut report,
        &mut ger,
    );
    assert_eq!(
        (ger.graphical_culture.as_deref(), ger.file_color, ger.use_legacy_ai_pp_spend),
        (Some("western_european_gfx"), Some(Color::Rgb([106, 119, 89])), Some(true))
    );

    countries::parse_colors(
        &src(
            "common/countries/colors.txt",
            "GER = { color = HSV { 0.1 0.15 0.4 } color_ui = rgb { 138 155 116 } }\nXYZ = { color = rgb { 1 2 3 } }\n",
        ),
        &mut report,
        &mut c,
    );
    let ger = c.get(tag("GER")).expect("GER");
    assert!(matches!(ger.map_color, Some(Color::Hsv(_))));
    assert_eq!(ger.ui_color, Some(Color::Rgb([138, 155, 116])));
    assert_eq!(report.count(&IssueKind::BrokenReference("country tag")), 1, "XYZ has no tag");
}

const STATE: &str = r#"
state={
	id=1
	name="STATE_1" # Corsica
	manpower = 322900
	state_category = town
	state_category = city
	history={
		owner = FRA
		victory_points = { 3838 1 }
		victory_points = { 9851 0.5 }
		buildings = {
			infrastructure = 2
			3838 = {
				naval_base = 3
				naval_headquarters = { level = 1 allowed = { has_dlc = "X" } }
			}
		}
		add_core_of = COR
		add_core_of = FRA
		1939.1.1 = { owner = ITA }
		set_variable = { x = 1 }
	}
	provinces={ 3838 9851 11804 }
	resources = { steel = 12 oil = 0.5 }
	local_supplies=0.0
	force_link_ownership_to = 334
	impassable_ignored_links = { 1024 }
	unknown_thing = 5
}
state = { name = "NO_ID" }
"#;

#[test]
fn states_static_data_and_plain_history() {
    let mut report = Report::default();
    let mut s = States::default();
    states::parse_file(&src("history/states/1-France.txt", STATE), &mut report, &mut s);
    assert_eq!(s.len(), 1);
    let st = s.get(StateId(1)).expect("state 1");
    assert_eq!((st.name.as_str(), st.manpower, st.category.as_deref()), ("STATE_1", Some(322_900), Some("town")));
    assert_eq!(st.provinces, vec![ProvinceId(3838), ProvinceId(9851), ProvinceId(11804)]);
    assert_eq!(st.resources, vec![("steel".to_string(), fixed("12")), ("oil".to_string(), fixed("0.5"))]);
    assert_eq!((st.local_supplies, st.force_link_ownership_to), (Some(Fixed::ZERO), Some(StateId(334))));
    assert_eq!(st.impassable_ignored_links, vec![StateId(1024)]);
    let h = &st.history;
    assert_eq!(h.owner, Some(tag("FRA")));
    assert_eq!(h.cores, vec![tag("COR"), tag("FRA")]);
    assert_eq!(h.victory_points, vec![(ProvinceId(3838), fixed("1")), (ProvinceId(9851), fixed("0.5"))]);
    assert_eq!(h.buildings.state.len(), 1);
    let naval = &h.buildings.provinces[&ProvinceId(3838)];
    assert_eq!(naval.len(), 2);
    assert!(naval[1].conditional && naval[1].level == 1 && naval[1].building == "naval_headquarters");
    assert_eq!(h.dated.len(), 1);
    // Reported: the duplicate category, the unknown key, the missing id; deferred: date, effect, building trigger.
    assert_eq!(report.count(&IssueKind::Duplicate), 1);
    assert_eq!(report.count(&IssueKind::UnknownKey), 1);
    assert_eq!(report.count(&IssueKind::Missing("id")), 1);
    assert_eq!(report.count(&IssueKind::Deferred), 3);
}

#[test]
fn first_state_definition_wins_in_evaluation_order() {
    let game = Scratch::new("states");
    game.write("history/states/123-XYZ.txt", "state = { id = 123 name = SECOND provinces = { 1 } }");
    game.write("history/states/123-ABC.txt", "state = { id = 123 name = FIRST provinces = { 1 } }");
    game.write("history/states/2-apple.txt", "state = { id = 2 name = lower provinces = { 2 } }");
    game.write("history/states/2-Banana.txt", "state = { id = 2 name = UPPER provinces = { 2 } }");
    let vfs = vfs::Vfs::new(vec![vfs::Layer::new("base", game.path())]);
    let order: Vec<String> = evaluation_order(vfs.list("history/states"))
        .iter()
        .map(|e| e.path.rsplit('/').next().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        order,
        vec!["123-ABC.txt", "123-XYZ.txt", "2-Banana.txt", "2-apple.txt"],
        "ASCII: digits, then upper case, then lower case"
    );
    let mut report = Report::default();
    let s = states::load(&vfs, &mut report);
    assert_eq!(s.get(StateId(123)).map(|d| d.name.as_str()), Some("FIRST"));
    assert_eq!(s.get(StateId(2)).map(|d| d.name.as_str()), Some("UPPER"));
}

#[test]
fn localisation_lines_and_precedence() {
    assert_eq!(localisation::parse_line(r#"GER_fascism:0 "German Reich""#), Some(("GER_fascism", "German Reich")));
    assert_eq!(localisation::parse_line(r#"key: "no version""#), Some(("key", "no version")));
    assert_eq!(localisation::parse_line(r#"k:0 "say "hi" now" # comment"#), Some(("k", r#"say "hi" now"#)));
    assert_eq!(localisation::parse_line(r#"k:0 """#), Some(("k", "")));
    for bad in ["k:0 no quotes", "two words:0 \"x\"", ":0 \"x\"", "k:abc \"x\"", "k:0 \"open"] {
        assert_eq!(localisation::parse_line(bad), None, "{bad}");
    }

    let game = Scratch::new("loc");
    game.write("localisation/english/a_l_english.yml", "\u{feff}l_english:\n KEY:0 \"from a\"\n ONLY_A:0 \"a\"\n");
    game.write("localisation/english/b_l_english.yml", "l_english:\n KEY:0 \"from b\"\n # comment\n\n");
    game.write("localisation/english/replace/z_l_english.yml", "l_english:\n KEY:0 \"replaced\"\n");
    game.write("localisation/russian/a_l_russian.yml", "l_russian:\n KEY:0 \"русский\"\n");
    game.write("localisation/english/noheader_l_english.yml", " OTHER:0 \"x\"\n");
    let vfs = vfs::Vfs::new(vec![vfs::Layer::new("base", game.path())]);
    let mut report = Report::default();
    let en: Localisation = localisation::load(&vfs, "english", &mut report);
    assert_eq!(en.get("KEY"), Some("replaced"), "replace/ wins");
    assert_eq!(en.get("ONLY_A"), Some("a"));
    assert_eq!(en.get("OTHER"), Some("x"));
    assert_eq!(report.count(&IssueKind::Missing("language header")), 1);
    let ru = localisation::load(&vfs, "russian", &mut report);
    assert_eq!(ru.get("KEY"), Some("русский"));
    assert_eq!(ru.len(), 1);
}

#[test]
fn whole_database_from_a_tiny_game_with_reference_checks() {
    let game = Scratch::new("db");
    game.write("map/default.map", "definitions = \"definition.csv\"\n");
    game.write("map/definition.csv", "1;1;1;1;land;false;plains;1\n2;2;2;2;land;false;volcano;1\n");
    game.write("common/terrain/00_terrain.txt", "categories = { plains = { movement_cost = 1 } }\n");
    game.write("common/country_tags/00_countries.txt", "AAA = \"countries/A.txt\"\n");
    game.write("common/countries/A.txt", "color = { 1 2 3 }\n");
    game.write("history/states/1-a.txt", "state = { id = 1 provinces = { 1 2 3 } history = { owner = AAA add_core_of = BBB victory_points = { 9 5 } } }\n");
    game.write("history/states/2-b.txt", "state = { id = 2 provinces = { 2 } }\n");
    let vfs = vfs::Vfs::new(vec![vfs::Layer::new("base", game.path())]);
    let (db, report) = Database::load(&vfs, &LoadOptions::default());
    assert_eq!((db.provinces.len(), db.states.len(), db.countries.len()), (2, 2, 1));
    let broken = |what: &'static str| report.count(&IssueKind::BrokenReference(what));
    assert_eq!(broken("terrain"), 1, "volcano");
    assert_eq!(broken("province"), 1, "province 3");
    assert_eq!(broken("country tag"), 1, "BBB");
    assert_eq!(broken("province of this state"), 1, "victory point in province 9");
    assert_eq!(report.count(&IssueKind::Redefinition("province in two states")), 1, "province 2");
}

#[test]
fn empty_or_missing_game_does_not_panic() {
    let vfs = vfs::Vfs::new(vec![vfs::Layer::new("ghost", "Z:/no/such/dir")]);
    let (db, report) = Database::load(&vfs, &LoadOptions::default());
    assert!(db.provinces.is_empty() && db.states.is_empty() && db.countries.is_empty());
    assert!(report.issues.iter().any(|i| matches!(i.kind, IssueKind::Missing(_))));
}

#[test]
fn resources_in_definition_order() {
    let mut report = Report::default();
    let mut r = Resources::default();
    resources::parse_file(
        &src(
            "common/resources/00_resources.txt",
            "resources = {\n oil = { icon_frame = 1 cic = 0.125 convoys = 0.1 }\n steel = { icon_frame = 5 }\n oil = { icon_frame = 9 }\n}\n",
        ),
        &mut report,
        &mut r,
    );
    assert_eq!(r.list.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["oil", "steel"]);
    let oil = r.get("oil").expect("oil");
    assert_eq!((oil.icon_frame, oil.cic, oil.convoys), (Some(1), Some(fixed("0.125")), Some(fixed("0.1"))));
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);
}

const SUB_UNITS: &str = r#"
sub_units = {
    infantry = {
        abbreviation = "INF"
        sprite = infantry
        map_icon_category = infantry
        priority = 600
        active = no
        type = { infantry }
        group = infantry
        categories = { category_front_line category_all_infantry }
        combat_width = 2
        max_strength = 25
        max_organisation = 60
        default_morale = 0.3
        manpower = 1000
        weight = 0.5
        weight = 0.7
        need = { infantry_equipment = 100 }
        forest = { attack = -0.2 movement = -0.4 }
        river = { attack = 0.4 }
        battalion_mult = { category = category_all_infantry add = yes breakthrough = 0.15 }
        deployed_leader_modifiers = { x = 1 }
        mystery = yes
    }
    fighter = { type = fighter sprite = light_plane }
}
"#;

#[test]
fn sub_units_keep_numbers_terrain_and_flags() {
    let mut report = Report::default();
    let mut u = Units::default();
    units::parse_categories(
        &src(
            "common/unit_tags/00_categories.txt",
            "sub_unit_categories = { category_front_line category_all_infantry }",
        ),
        &mut report,
        &mut u,
    );
    units::parse_sub_units(&src("common/units/infantry.txt", SUB_UNITS), &mut report, &mut u);
    assert_eq!(u.categories, vec!["category_front_line", "category_all_infantry"]);
    let inf = &u.sub_units["infantry"];
    assert_eq!(inf.types, vec!["infantry"]);
    assert_eq!(u.sub_units["fighter"].types, vec!["fighter"], "a single type without braces");
    assert_eq!(inf.need, vec![("infantry_equipment".to_string(), fixed("100"))]);
    assert_eq!((inf.number("max_strength"), inf.number("default_morale")), (Some(fixed("25")), Some(fixed("0.3"))));
    assert_eq!(inf.number("weight"), Some(fixed("0.5")), "first of two");
    assert_eq!(inf.flags, vec![("active".to_string(), false)]);
    assert_eq!(
        inf.terrain["forest"],
        vec![("attack".to_string(), fixed("-0.2")), ("movement".to_string(), fixed("-0.4"))]
    );
    assert!(inf.terrain.contains_key("river"));
    assert_eq!(inf.battalion_mult.len(), 1);
    assert!(inf.battalion_mult[0].add);
    assert_eq!(inf.battalion_mult[0].values, vec![("breakthrough".to_string(), fixed("0.15"))]);
    assert_eq!(report.count(&IssueKind::Duplicate), 1, "second weight");
    assert_eq!(report.count(&IssueKind::UnknownKey), 1, "mystery");
    assert_eq!(report.count(&IssueKind::Deferred), 1, "deployed_leader_modifiers");
}

const EQUIPMENT: &str = r#"
equipments = {
    infantry_equipment = {
        year = 1918
        is_archetype = yes
        is_buildable = no
        type = infantry
        group_by = archetype
        soft_attack = 3
        hard_attack = 0.5
        build_cost_ic = 0.43
        resources = { steel = 1 }
        can_be_produced = { always = yes }
    }
    infantry_equipment_1 = {
        year = 1936
        archetype = infantry_equipment
        priority = 10
        upgrades = { up_a up_b }
        type = { infantry support }
        soft_attack = 4
    }
}
duplicate_archetypes = {
    copy_equipment = { archetype = infantry_equipment }
}
search_filters = { x = 1 }
"#;

#[test]
fn equipment_archetypes_variants_and_duplicates() {
    let mut report = Report::default();
    let mut e = Equipment::default();
    equipment::parse_file(&src("common/units/equipment/infantry.txt", EQUIPMENT), &mut report, &mut e);
    assert_eq!(e.len(), 3);
    let arch = e.get("infantry_equipment").expect("archetype");
    assert!(arch.is_archetype() && arch.flag("is_buildable") == Some(false));
    assert_eq!((arch.number("soft_attack"), arch.number("build_cost_ic")), (Some(fixed("3")), Some(fixed("0.43"))));
    assert_eq!(arch.resources, vec![("steel".to_string(), fixed("1"))]);
    let v1 = e.get("infantry_equipment_1").expect("variant");
    assert_eq!(v1.text("archetype"), Some("infantry_equipment"));
    assert_eq!(v1.types, vec!["infantry".to_string(), "support".to_string()]);
    assert_eq!(v1.upgrades.len(), 2);
    assert!(!v1.is_archetype());
    assert!(e.get("copy_equipment").is_some_and(|c| c.duplicate && c.is_archetype()));
    assert_eq!(report.count(&IssueKind::Deferred), 2, "can_be_produced and search_filters");
    assert_eq!(report.count(&IssueKind::UnknownKey), 0);
}

#[test]
fn unit_and_equipment_references_are_checked() {
    let game = Scratch::new("units");
    game.write("common/terrain/00_terrain.txt", "categories = { forest = { } }");
    game.write("common/resources/00_resources.txt", "resources = { steel = { } }");
    game.write("common/unit_tags/00_categories.txt", "sub_unit_categories = { category_a }");
    game.write(
        "common/units/equipment/eq.txt",
        "equipments = { gun = { is_archetype = yes resources = { steel = 1 gold = 2 } } gun_1 = { archetype = gun parent = nope } bad_1 = { archetype = gun_1 } }",
    );
    game.write(
        "common/units/u.txt",
        "sub_units = { inf = { categories = { category_a category_b } need = { gun = 1 laser = 2 } forest = { attack = 1 } swamp = { attack = 1 } river = { attack = 1 } battalion_mult = { category = category_z } } }",
    );
    let vfs = vfs::Vfs::new(vec![vfs::Layer::new("base", game.path())]);
    let (_, report) = Database::load(&vfs, &LoadOptions::default());
    let broken = |what: &'static str| report.count(&IssueKind::BrokenReference(what));
    assert_eq!(broken("resource"), 1, "gold");
    assert_eq!(broken("equipment"), 2, "parent nope, need laser");
    assert_eq!(broken("archetype (not marked is_archetype)"), 1, "bad_1 -> gun_1");
    assert_eq!(broken("sub-unit category"), 2, "category_b and battalion_mult category_z");
    assert_eq!(broken("terrain"), 1, "swamp; river is a combat situation");
}

/// The real installation (skipped without `HOI4_DIR`).
#[test]
fn real_installation_loads_with_full_key_coverage() {
    let Ok(dir) = std::env::var("HOI4_DIR") else { return };
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let (db, report) = Database::load(&vfs, &LoadOptions::default());
    assert!(db.provinces.len() > 13_000, "{}", db.provinces.len());
    assert!(db.states.len() > 1_000, "{}", db.states.len());
    assert!(db.countries.len() > 400, "{}", db.countries.len());
    assert!(db.terrain.get("forest").is_some() && db.terrain.get("ocean").is_some_and(|o| o.is_water));
    assert!(db.localisation.len() > 100_000);
    let ger = db.countries.get(tag("GER")).expect("GER");
    assert!(ger.file_color.is_some() && ger.ui_color.is_some());
    let corsica = db.states.get(StateId(1)).expect("state 1");
    assert_eq!(corsica.history.owner, Some(tag("FRA")));
    assert!(db.localisation.get("GER_fascism").is_some());
    assert!(db.units.sub_units.len() > 100 && db.equipment.len() > 300 && db.resources.len() >= 6);
    assert!(db.units.sub_units["infantry"].need.iter().any(|(k, _)| k == "infantry_equipment"));
    assert_eq!(db.equipment.get("infantry_equipment_1").and_then(|e| e.text("archetype")), Some("infantry_equipment"));
    // Every key in the loaded folders is understood: nothing unknown, no broken references.
    let unknown: Vec<String> =
        report.issues.iter().filter(|i| i.kind == IssueKind::UnknownKey).map(|i| i.to_string()).take(5).collect();
    assert!(unknown.is_empty(), "{unknown:?}");
    assert_eq!(report.issues.iter().filter(|i| matches!(i.kind, IssueKind::BrokenReference(_))).count(), 0);
}
