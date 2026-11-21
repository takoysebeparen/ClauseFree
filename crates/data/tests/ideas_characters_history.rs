use std::path::Path;

use common::Fixed;
use data::characters::{self, Characters, CommanderKind};
use data::country_history::{self, CountryHistories};
use data::equipment_groups::{self, EquipmentGroups};
use data::ideas::{self, Ideas};
use data::report::{IssueKind, Report};
use data::source::Source;
use data::states::StateId;
use data::{Database, Date, LoadOptions, Tag};

fn fixed(text: &str) -> Fixed {
    Fixed::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn pair(k: &str, v: &str) -> (String, Fixed) {
    (k.to_string(), fixed(v))
}

fn src(path: &str, text: &str) -> Source {
    Source::from_text(path, text.to_string())
}

fn tag(text: &str) -> Tag {
    Tag::parse(text).unwrap_or_else(|| panic!("bad tag {text}"))
}

const IDEA_TAGS: &str = r#"
idea_categories = {
	country = { type = national_spirit cost = 0 removal_cost = -1 ledger = hidden }
	government = {
		slot = mobilization_laws
		slot = economy
		character_slot = political_advisor
		character_slot = political_advisor
		ledger = civilian
		cost = 150
		removal_cost = 0
	}
	army_spirit = { type = army_spirit politics_tab = no }
	hidden_ideas = { hidden = yes }
	country = { ledger = army }
}
slot_ledgers = {
	army_chief = army
	theorist = invalid # must be overridden
	army_chief = navy
}
"#;

const IDEAS: &str = r#"
ideas = {
	economy = {
		law = yes
		civilian_economy = {
			level = 1
			default = yes
			cost = 150
			removal_cost = -1
			available = { has_war_support > 0.25 }
			modifier = {
				consumer_goods_factor = 0.35
				custom_modifier_tooltip = civilian_economy_tt
				hidden_modifier = { ai_join_ally_desire_factor = -500 }
			}
			ai_will_do = { factor = 1 }
		}
	}
	country = {
		GER_rearmament = {
			name = GER_other_name
			picture = generic_production_bonus
			picture = second_picture
			allowed_civil_war = { always = yes }
			targeted_modifier = { tag = SOV attack_bonus_against = 0.05 }
			targeted_modifier = { fascism_drift = 0.15 }
			research_bonus = { armor = 0.15 }
			equipment_bonus = {
				infantry_equipment = { build_cost_ic = -0.15 instant = yes reliability = -0.1 instant = yes }
			}
			equipment_bonus = { armor = { soft_attack = 0.03 } }
			rule = { can_send_volunteers = yes desc = rule_desc_key can_join_factions = no }
			cancel_if_invalid = no
			on_add = { add_political_power = 10 }
		}
	}
	tank_manufacturer = {
		GER_henschel = {
			traits = { heavy_tank_manufacturer }
		}
		designer = yes
		designer = yes
	}
}
ideas = {
	country = {
		GER_rearmament = { }
	}
}
"#;

fn load_ideas() -> (Ideas, Report) {
    let mut report = Report::default();
    let mut out = Ideas::default();
    ideas::parse_tags(&src("common/idea_tags/00_idea.txt", IDEA_TAGS), &mut report, &mut out);
    ideas::parse_file(&src("common/ideas/GER.txt", IDEAS), &mut report, &mut out);
    (out, report)
}

#[test]
fn idea_categories_slots_and_ledgers() {
    let (ideas, report) = load_ideas();
    assert_eq!(ideas.categories.len(), 4, "the repeated `country` is not added");
    let gov = ideas.category("government").expect("government");
    assert_eq!(gov.slots, ["mobilization_laws", "economy"]);
    assert_eq!(gov.character_slots, ["political_advisor", "political_advisor"], "repeats are second slots");
    assert_eq!((gov.cost, gov.removal_cost), (Some(fixed("150")), Some(fixed("0"))));
    assert_eq!(ideas.category("country").and_then(|c| c.kind.as_deref()), Some("national_spirit"));
    assert_eq!(ideas.category("army_spirit").and_then(|c| c.politics_tab), Some(false));
    assert!(ideas.category("hidden_ideas").is_some_and(|c| c.hidden));
    assert_eq!(ideas.category_of_slot("economy").map(|c| c.name.as_str()), Some("government"));
    assert!(ideas.has_character_slot("political_advisor") && !ideas.has_character_slot("economy"));
    assert_eq!(ideas.slot_ledgers, [("army_chief".into(), "army".into()), ("theorist".into(), "invalid".into())]);
    // Repeated category, repeated slot ledger, repeated idea.
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 3);
}

#[test]
fn ideas_keep_values_bonuses_and_group_flags() {
    let (ideas, report) = load_ideas();
    assert_eq!(ideas.len(), 3);
    let law = ideas.get("civilian_economy").expect("law");
    assert_eq!(law.group, "economy");
    assert!(law.law && law.default && !law.designer);
    assert_eq!(law.level, Some(1));
    assert_eq!(law.modifiers.get("consumer_goods_factor"), Some(fixed("0.35")));
    assert_eq!(law.modifiers.custom_tooltips, ["civilian_economy_tt"]);
    assert_eq!(law.modifiers.hidden, [pair("ai_join_ally_desire_factor", "-500")]);
    assert!(law.has_script("available") && law.has_script("ai_will_do") && !law.has_script("on_add"));

    let spirit = ideas.get("GER_rearmament").expect("spirit");
    assert_eq!(spirit.file, "common/ideas/GER.txt");
    assert_eq!(spirit.name_key.as_deref(), Some("GER_other_name"));
    assert_eq!(spirit.picture.as_deref(), Some("generic_production_bonus"), "the first of two is kept");
    assert_eq!(spirit.targeted_modifiers.len(), 1, "the block without `tag` is reported");
    assert_eq!(spirit.targeted_modifiers[0].target, "SOV");
    assert_eq!(spirit.targeted_modifiers[0].modifiers.get("attack_bonus_against"), Some(fixed("0.05")));
    assert_eq!(spirit.research_bonus, [pair("armor", "0.15")]);
    assert_eq!(spirit.equipment_bonus.len(), 2, "both `equipment_bonus` blocks are kept");
    let inf = &spirit.equipment_bonus[0];
    assert_eq!(inf.target, "infantry_equipment");
    assert!(inf.instant, "`instant = yes` may repeat");
    assert_eq!(inf.stats, [pair("build_cost_ic", "-0.15"), pair("reliability", "-0.1")]);
    assert!(!spirit.equipment_bonus[1].instant);
    assert_eq!(spirit.rules, [("can_send_volunteers".to_string(), true), ("can_join_factions".to_string(), false)]);
    assert_eq!(spirit.rule_desc.as_deref(), Some("rule_desc_key"));
    assert_eq!(spirit.cancel_if_invalid, Some(false));
    assert!(spirit.has_script("allowed_civil_war") && spirit.has_script("on_add"));

    let designer = ideas.get("GER_henschel").expect("designer");
    assert!(designer.designer, "`designer = yes` after the ideas, and twice, still holds for the group");
    assert_eq!(designer.traits, ["heavy_tank_manufacturer"]);

    assert_eq!(report.count(&IssueKind::Missing("tag")), 1);
    assert_eq!(report.count(&IssueKind::Duplicate), 1, "the second `picture`");
    assert_eq!(report.count(&IssueKind::UnknownKey), 0);
}

const CHARACTERS: &str = r#"
characters = {
	GER_adolf_hitler = {
		name = GER_adolf_hitler
		portraits = { civilian = { large = GFX_portrait_GER_adolf_hitler } }
		portraits = { army = { small = GFX_small } }
		country_leader = { ideology = nazism traits = { GER_der_fuhrer } expire = "1965.1.1.1" id = -1 }
		country_leader = { ideology = despotism expire = "not a date" }
		can_be_captured = no
		gender = male
	}
	GER_erwin_rommel = {
		name = "Erwin Rommel"
		corps_commander = {
			traits = { panzer_leader }
			skill = 4
			attack_skill = 4
			defense_skill = 2
			planning_skill = 3
			logistics_skill = 3
			legacy_id = 701
			visible = { always = yes }
		}
		navy_leader = { skill = 1 maneuvering_skill = 2 coordination_skill = 3 }
		advisor = {
			slot = political_advisor
			idea_token = GER_rommel_advisor
			cost = 100
			allowed = { original_tag = GER }
			traits = { silent_workhorse }
			research_bonus = { armor = 0.1 }
			modifier = { stability_factor = 0.05 }
			ai_will_do = { factor = 1 }
		}
		scientist = {
			desc = rommel_desc
			traits = { brilliant }
			skills = { specialization_land = 2 }
			visible = { always = yes }
		}
		allowed_civil_war = { has_government = fascism }
	}
	AFG_mohammed_zahir_shah = {
		instance = {
			allowed = { NOT = { has_dlc = "Graveyard of Empires" } }
			name = AFG_mohammed_zahir_shah
			country_leader = { ideology = despotism }
		}
		instance = {
			name = AFG_mohammed_zahir_shah
			field_marshal = { skill = 3 }
		}
	}
	GER_adolf_hitler = { }
}
"#;

#[test]
fn characters_roles_portraits_and_instances() {
    let mut report = Report::default();
    let mut chars = Characters::default();
    characters::parse_file(&src("common/characters/GER.txt", CHARACTERS), &mut report, &mut chars);
    assert_eq!(chars.len(), 3);
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);
    assert_eq!(report.count(&IssueKind::InvalidValue("a date")), 1);
    assert_eq!(report.count(&IssueKind::UnknownKey), 0);

    let hitler = chars.get("GER_adolf_hitler").expect("hitler");
    let b = &hitler.body;
    assert_eq!(b.name.as_deref(), Some("GER_adolf_hitler"));
    assert_eq!(b.can_be_captured, Some(false));
    assert_eq!(b.gender.as_deref(), Some("male"));
    assert_eq!(b.portraits.len(), 2, "portraits split over two blocks are all kept");
    assert_eq!(b.portraits[0].kind, "civilian");
    assert_eq!(b.portraits[0].large.as_deref(), Some("GFX_portrait_GER_adolf_hitler"));
    assert_eq!(b.portraits[1].small.as_deref(), Some("GFX_small"));
    assert_eq!(b.country_leader.len(), 2);
    let leader = &b.country_leader[0];
    assert_eq!(leader.ideology.as_deref(), Some("nazism"));
    assert_eq!(leader.expire, Some(Date { year: 1965, month: 1, day: 1, hour: 1 }));
    assert_eq!((leader.id, leader.traits.as_slice()), (Some(-1), ["GER_der_fuhrer".to_string()].as_slice()));
    assert_eq!(b.country_leader[1].expire, None);

    let rommel = &chars.get("GER_erwin_rommel").expect("rommel").body;
    assert!(rommel.has_allowed_civil_war);
    assert_eq!(rommel.commanders.len(), 2);
    let corps = &rommel.commanders[0];
    assert_eq!(corps.kind, CommanderKind::CorpsCommander);
    assert_eq!((corps.skill("skill"), corps.skill("planning_skill")), (Some(4), Some(3)));
    assert_eq!((corps.legacy_id, corps.conditional), (Some(701), true));
    assert_eq!(corps.traits, ["panzer_leader"]);
    let navy = &rommel.commanders[1];
    assert_eq!(navy.kind, CommanderKind::NavyLeader);
    assert_eq!(navy.skill("coordination_skill"), Some(3));
    assert!(!navy.conditional);
    let adv = &rommel.advisors[0];
    assert_eq!(adv.slot.as_deref(), Some("political_advisor"));
    assert_eq!(adv.idea_token.as_deref(), Some("GER_rommel_advisor"));
    assert_eq!(adv.cost, Some(fixed("100")));
    assert_eq!(adv.research_bonus, [pair("armor", "0.1")]);
    assert_eq!(adv.modifiers.get("stability_factor"), Some(fixed("0.05")));
    assert_eq!(adv.scripts, ["allowed", "ai_will_do"]);
    let sci = rommel.scientist.as_ref().expect("scientist");
    assert_eq!((sci.desc.as_deref(), sci.conditional), (Some("rommel_desc"), true));
    assert_eq!(sci.skills, [pair("specialization_land", "2")]);

    let shah = chars.get("AFG_mohammed_zahir_shah").expect("shah");
    assert_eq!(shah.instances.len(), 2);
    assert!(shah.instances[0].conditional && !shah.instances[1].conditional);
    assert_eq!(shah.instances[0].body.country_leader[0].ideology.as_deref(), Some("despotism"));
    assert_eq!(shah.instances[1].body.commanders[0].kind, CommanderKind::FieldMarshal);
    assert_eq!(shah.bodies().count(), 3);
}

const HISTORY: &str = r#"
capital = 64 # Berlin
oob = "GER_1936"
set_research_slots = 4
set_convoys = 10
set_stability = 0.5
set_war_support = 0.3
starting_train_buffer = 1.5
set_technology = { infantry_weapons = 1 popup = no }
set_technology = { tech_support = 1 }
set_politics = { ruling_party = fascism last_election = "1933.3.5" election_frequency = 48 elections_allowed = no }
set_popularities = { democratic = 35 fascism = 65 }
add_ideas = { GER_rearmament civilian_economy }
add_ideas = GER_rommel_advisor
recruit_character = GER_adolf_hitler
recruit_character = GER_erwin_rommel
set_grand_doctrine = mobile_warfare
set_grand_doctrine = fleet_in_being
set_sub_doctrine = assault_infantry
set_convoys = 20
if = { limit = { has_dlc = "No Step Back" } set_oob = "GER_1936_nsb" }
set_oob = GER_1936_final
set_variable = { x = 1 }
1939.1.1 = { set_stability = 0.9 }
"#;

#[test]
fn country_history_plain_effects_last_one_wins() {
    let mut report = Report::default();
    let mut out = CountryHistories::default();
    country_history::parse_file(&src("history/countries/GER - Germany.txt", HISTORY), &mut report, &mut out);
    let h = out.get(tag("GER")).expect("GER");
    assert_eq!(h.capital, Some(StateId(64)));
    assert_eq!(h.oob.as_deref(), Some("GER_1936_final"), "the plain `set_oob` after `oob` replaces it");
    assert_eq!((h.research_slots, h.convoys), (Some(4), Some(20)), "the later `set_convoys` wins");
    assert_eq!((h.stability, h.war_support), (Some(fixed("0.5")), Some(fixed("0.3"))));
    assert_eq!(h.starting_train_buffer, Some(fixed("1.5")));
    assert_eq!(h.technologies, [pair("infantry_weapons", "1"), pair("tech_support", "1")]);
    let p = h.politics.as_ref().expect("politics");
    assert_eq!(p.ruling_party.as_deref(), Some("fascism"));
    assert_eq!(p.last_election, Some(Date { year: 1933, month: 3, day: 5, hour: 0 }));
    assert_eq!((p.election_frequency, p.elections_allowed), (Some(48), Some(false)));
    assert_eq!(h.popularities, [pair("democratic", "35"), pair("fascism", "65")]);
    assert_eq!(h.ideas, ["GER_rearmament", "civilian_economy", "GER_rommel_advisor"]);
    assert_eq!(h.characters, ["GER_adolf_hitler", "GER_erwin_rommel"]);
    assert_eq!(h.grand_doctrines, ["mobile_warfare", "fleet_in_being"]);
    assert_eq!(h.sub_doctrines, ["assault_infantry"]);
    assert_eq!(h.dated, [Date { year: 1939, month: 1, day: 1, hour: 0 }]);
    assert_eq!(report.count(&IssueKind::UnknownKey), 0);
    // `if`, `set_variable` and the dated block go to the script VM.
    assert_eq!(report.count(&IssueKind::Deferred), 3);
    assert_eq!(report.errors().count(), 0, "{:?}", report.issues);
}

#[test]
fn country_history_files_tags_variables_and_duplicates() {
    assert_eq!(country_history::tag_of_file("history/countries/HBC  - Hebei-Chahar.txt"), Some(tag("HBC")));
    assert_eq!(country_history::tag_of_file("history/countries/KLT -Kalat.txt"), Some(tag("KLT")));
    assert_eq!(country_history::tag_of_file("history/countries/ab.txt"), None);
    let mut report = Report::default();
    let mut out = CountryHistories::default();
    let popularities = "set_popularities = { democratic = PER_democratic_popularity fascism = 0 }";
    country_history::parse_file(&src("history/countries/PER - Persia.txt", popularities), &mut report, &mut out);
    country_history::parse_file(&src("history/countries/PER - Other.txt", "capital = 1"), &mut report, &mut out);
    country_history::parse_file(&src("history/countries/x.txt", "capital = 1"), &mut report, &mut out);
    let per = out.get(tag("PER")).expect("PER");
    assert!(per.popularities.is_empty(), "variables are resolved by the script VM");
    assert_eq!(per.capital, None, "the second file for the tag is not read");
    assert_eq!(out.len(), 1);
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);
    assert_eq!(report.count(&IssueKind::InvalidValue("a file name starting with a country tag")), 1);
    assert_eq!(report.count(&IssueKind::Deferred), 1);
}

#[test]
fn equipment_groups_list_members() {
    let mut report = Report::default();
    let mut out = EquipmentGroups::default();
    let text = r#"
mio_cat_eq_all_light_tank = { equipment_type = { light_tank_chassis amphibious_tank_chassis } }
useless_junk = { description = "Useless Junk" equipment_type = { mio_cat_eq_all_light_tank carrier } }
useless_junk = { }
"#;
    equipment_groups::parse_file(&src("common/equipment_groups/a.txt", text), &mut report, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out.get("mio_cat_eq_all_light_tank").map(|g| g.members.len()), Some(2));
    let junk = out.get("useless_junk").expect("junk");
    assert_eq!(junk.description.as_deref(), Some("Useless Junk"));
    assert_eq!(junk.members, ["mio_cat_eq_all_light_tank", "carrier"]);
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);
}

#[test]
fn idea_character_and_history_references_are_checked() {
    let (ideas, _) = load_ideas();
    let mut report = Report::default();
    let mut chars = Characters::default();
    characters::parse_file(&src("common/characters/GER.txt", CHARACTERS), &mut report, &mut chars);
    let mut groups = EquipmentGroups::default();
    equipment_groups::parse_file(
        &src("common/equipment_groups/a.txt", "g = { equipment_type = { infantry_equipment ghost } }"),
        &mut report,
        &mut groups,
    );
    let mut histories = CountryHistories::default();
    country_history::parse_file(&src("history/countries/GER - Germany.txt", HISTORY), &mut report, &mut histories);
    let db = Database {
        ideas,
        characters: chars,
        equipment_groups: groups,
        country_history: histories,
        ..Default::default()
    };
    let mut refs = Report::default();
    db.check_references(&mut refs);
    let count = |what| refs.count(&IssueKind::BrokenReference(what));
    // Nothing else is loaded: no equipment, technologies, countries, states or OOBs.
    assert_eq!(count("idea category or slot"), 1, "`tank_manufacturer` is not declared by the test categories");
    assert_eq!(count("technology category"), 2, "`armor` in the idea and in the advisor");
    assert_eq!(count("equipment, equipment type or group"), 4, "two idea bonuses, two group members");
    assert_eq!(count("character slot"), 0);
    assert_eq!(count("country tag"), 1);
    assert_eq!(count("state"), 1);
    assert_eq!(count("OOB file"), 1);
    assert_eq!(count("technology"), 2);
    assert_eq!(count("idea"), 0, "`GER_rommel_advisor` is an advisor's idea token");
    assert_eq!(count("character"), 0);
}

/// The real installation (skipped without `HOI4_DIR`).
#[test]
fn real_ideas_characters_and_history() {
    let Ok(dir) = std::env::var("HOI4_DIR") else { return };
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let (db, report) = Database::load(&vfs, &LoadOptions::default());
    assert!(db.ideas.len() > 6000, "{}", db.ideas.len());
    assert!(db.ideas.category("government").is_some_and(|c| c.slots.iter().any(|s| s == "economy")));
    let law = db.ideas.get("partial_economic_mobilisation").expect("law");
    assert!(law.law && law.group == "economy");
    assert!(db.characters.len() > 5000, "{}", db.characters.len());
    let hitler = db.characters.get("GER_adolf_hitler").expect("hitler");
    assert_eq!(hitler.body.country_leader[0].ideology.as_deref(), Some("nazism"));
    assert!(db.equipment_groups.len() > 20);
    assert!(db.country_history.len() > 350, "{}", db.country_history.len());
    let ger = db.country_history.get(tag("GER")).expect("GER history");
    assert_eq!(ger.capital, Some(StateId(64)));
    assert_eq!((ger.research_slots, ger.convoys), (Some(4), Some(200)));
    assert_eq!(ger.politics.as_ref().and_then(|p| p.ruling_party.as_deref()), Some("fascism"));
    assert!(ger.characters.iter().any(|c| c == "GER_adolf_hitler"));
    for prefix in ["idea", "character", "country_history", "equipment_group"] {
        let unknown =
            report.issues.iter().filter(|i| i.path.starts_with(prefix) && i.kind == IssueKind::UnknownKey).count();
        assert_eq!(unknown, 0, "{prefix}");
    }
}
