use std::path::Path;

use common::Fixed;
use data::country_leader::{self, CountryLeaderTraits};
use data::doctrines::{self, Doctrines, Kind};
use data::report::{IssueKind, Report};
use data::source::Source;
use data::{Database, LoadOptions};

fn fixed(text: &str) -> Fixed {
    Fixed::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn src(path: &str, text: &str) -> Source {
    Source::from_text(path, text.to_string())
}

const LEADER_TRAITS: &str = r#"
@tier3_cp_cap_increase = 30

leader_traits = {
	dictator = {
		random = no
		sprite = 13
		political_power_factor = 0.25
		command_cap_increase = @tier3_cp_cap_increase
		custom_modifier_tooltip = dictator_tt
		hidden_modifier = { ai_focus_aggressive_factor = 0.5 }
		targeted_modifier = { tag = GER attack_bonus_against = 0.1 }
		equipment_bonus = { infantry_equipment = { soft_attack = 0.05 instant = yes } }
		ai_strategy = { type = antagonize id = "GER" value = 200 }
		ai_will_do = { factor = 1 }
	}
	dictator = { random = yes }
	broken = {
		command_power = @nowhere
		strange = { a = 1 }
		ai_strategy = { id = SOV }
	}
}
"#;

#[test]
fn country_leader_traits() {
    let mut report = Report::default();
    let mut out = CountryLeaderTraits::default();
    country_leader::parse_file(&src("common/country_leader/00_traits.txt", LEADER_TRAITS), &mut report, &mut out);
    assert_eq!(out.len(), 2, "the redefinition of dictator is not kept");
    let t = out.get("dictator").expect("dictator");
    assert_eq!((t.random, t.sprite), (Some(false), Some(13)));
    assert_eq!(t.modifiers.get("political_power_factor"), Some(fixed("0.25")));
    assert_eq!(t.modifiers.get("command_cap_increase"), Some(fixed("30")), "through the file's constant");
    assert_eq!(t.modifiers.custom_tooltips, vec!["dictator_tt".to_string()]);
    assert_eq!(t.modifiers.hidden, vec![("ai_focus_aggressive_factor".to_string(), fixed("0.5"))]);
    assert_eq!(t.targeted_modifiers[0].target, "GER");
    assert_eq!(t.equipment_bonus[0].target, "infantry_equipment");
    assert!(t.equipment_bonus[0].instant);
    let a = &t.ai_strategies[0];
    assert_eq!((a.kind.as_str(), a.id.as_deref(), a.value), ("antagonize", Some("GER"), Some(fixed("200"))));
    assert!(t.has_ai_will_do);
    assert_eq!(report.count(&IssueKind::Deferred), 1, "ai_will_do");
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1);
    assert_eq!(report.count(&IssueKind::BrokenReference("constant")), 1, "@nowhere");
    assert_eq!(report.count(&IssueKind::UnknownKey), 1, "strange");
    assert_eq!(report.count(&IssueKind::Missing("type")), 1, "ai_strategy without a type");
}

const FOLDERS: &str = r#"
land = {
    allowed = { always = yes }
    name = "land_doctrine_folder"
    ledger = army
    ledger_gfx = GFX_land
    tab_gfx = GFX_land_tab
    color_frame = 1
    sound = ui_doctrine_tab_land
}
"#;

const GRAND: &str = r#"
mobile_warfare = {
    folder = land
    name = GRAND_DOCTRINE_MOBILE_WARFARE
    description = GRAND_DOCTRINE_MOBILE_WARFARE_DESC
    icon = GFX_mobile_warfare
    available = { always = yes }
    xp_cost = 100
    xp_type = army
    ai_will_do = { base = 1 }
    tracks = { infantry armor }
    max_track_rows = 2
    planning_speed = 0.20
    enable_tactic = tactic_unexpected_thrust
    category_tanks = { max_organisation = 1 }
    milestones = {
        { org_loss_when_moving = -0.15 effect = { set_variable = { x = 1 } } }
        { category_army = { breakthrough = 0.15 } }
    }
}
short = {
    folder = land
    tracks = { infantry armor }
    milestones = { { planning_speed = 0.1 } }
}
"#;

const TRACKS: &str = r#"
infantry = {
    name = DOCTRINE_TRACK_INFANTRY
    background = "GFX_bg"
    background_offset = 10
    icon = "GFX_icon"
    icon_frame = "GFX_frame"
    active = { has_political_power = 100 }
    mastery = {
        multiplier = 2.0
        categories = { category_all_infantry category_cavalry }
        equipment = { screen }
    }
}
"#;

const SUBDOCTRINES: &str = r#"
mobile_infantry = {
    track = infantry
    name = SUBDOCTRINE_MOBILE_INFANTRY
    description = SUBDOCTRINE_MOBILE_INFANTRY_DESC
    icon = GFX_mobile_infantry
    xp_cost = 100
    xp_type = army
    available = { always = yes }
    ai_will_do = { base = 1 modifier = { factor = 0.5 is_major = no } }
    enable_tactic = tactic_overwhelming_fire
    category_all_infantry = { breakthrough = 0.1 }
    modifiers = { army_speed_factor = 0.05 }
    rewards = {
        cavalry_charge = {
            mastery = 150
            category_cavalry = { breakthrough = 0.20 }
        }
        refits = { modifier = { refit_speed = 0.15 } }
        cavalry_charge = { }
    }
    xor = { other_one }
    mastery = { sub_units = { bicycle_battalion } }
}
both = {
    track = { infantry armor }
    allow_in_multiple_tracks = yes
    equipment_bonus = { capital_ship = { naval_range = 0.10 instant = yes } }
}
lost = { name = X }
"#;

#[test]
fn doctrines_folders_tracks_and_rewards() {
    let mut report = Report::default();
    let mut out = Doctrines::default();
    for (kind, path, text) in [
        (Kind::Folders, "common/doctrines/folders/doctrine_folders.txt", FOLDERS),
        (Kind::GrandDoctrines, "common/doctrines/grand_doctrines/land.txt", GRAND),
        (Kind::Tracks, "common/doctrines/tracks/land.txt", TRACKS),
        (Kind::Subdoctrines, "common/doctrines/subdoctrines/land/infantry.txt", SUBDOCTRINES),
    ] {
        doctrines::parse_file(kind, &src(path, text), &mut report, &mut out);
    }
    let land = out.folder("land").expect("land");
    assert_eq!((land.ledger.as_deref(), land.color_frame, land.has_allowed), (Some("army"), Some(1), true));

    let g = out.grand_doctrine("mobile_warfare").expect("mobile_warfare");
    assert_eq!(
        (g.folder.as_deref(), g.xp_cost, g.xp_type.as_deref()),
        (Some("land"), Some(fixed("100")), Some("army"))
    );
    assert_eq!(g.tracks, vec!["infantry".to_string(), "armor".to_string()]);
    assert_eq!(g.max_track_rows, Some(2));
    assert_eq!(g.effects.modifiers.get("planning_speed"), Some(fixed("0.20")));
    assert_eq!(g.effects.tactics, vec!["tactic_unexpected_thrust".to_string()]);
    let tanks = g.effects.unit_stats("category_tanks").expect("category_tanks");
    assert_eq!(tanks.stats, vec![("max_organisation".to_string(), fixed("1"))]);
    assert_eq!(g.milestones.len(), 2);
    assert!(g.milestones[0].has_effect);
    assert_eq!(g.milestones[0].modifiers.get("org_loss_when_moving"), Some(fixed("-0.15")));
    assert!(g.milestones[1].unit_stats("category_army").is_some());
    assert_eq!(g.scripts, vec!["available".to_string(), "ai_will_do".to_string()]);

    let t = out.track("infantry").expect("infantry");
    assert_eq!((t.background_offset, t.has_active), (Some(fixed("10")), true));
    let m = t.mastery.as_ref().expect("mastery");
    assert_eq!((m.multiplier, m.categories.len(), m.equipment.len()), (Some(fixed("2.0")), 2, 1));

    let s = out.subdoctrine("mobile_infantry").expect("mobile_infantry");
    assert_eq!(s.tracks, vec!["infantry".to_string()]);
    assert_eq!(s.effects.modifiers.get("army_speed_factor"), Some(fixed("0.05")));
    assert!(s.effects.unit_stats("category_all_infantry").is_some());
    assert_eq!(s.rewards.len(), 2, "the second cavalry_charge is not kept");
    assert_eq!((s.rewards[0].name.as_str(), s.rewards[0].mastery), ("cavalry_charge", Some(fixed("150"))));
    assert_eq!(s.rewards[1].effects.modifiers.get("refit_speed"), Some(fixed("0.15")), "`modifier` spelling");
    assert_eq!(s.xor, vec!["other_one".to_string()]);
    assert_eq!(s.mastery.as_ref().map(|m| m.sub_units.len()), Some(1));
    let both = out.subdoctrine("both").expect("both");
    assert_eq!((both.tracks.len(), both.allow_in_multiple_tracks), (2, Some(true)));
    assert_eq!(both.effects.equipment_bonus[0].target, "capital_ship");

    assert_eq!(out.all_effects().count(), 10, "grand doctrines 2 + milestones 3, subdoctrines 3 + rewards 2");
    assert_eq!(report.count(&IssueKind::InvalidValue("one milestone per track")), 1, "short");
    assert_eq!(report.count(&IssueKind::Missing("track")), 1, "lost");
    assert_eq!(report.count(&IssueKind::Redefinition("first")), 1, "reward");
    // available, ai_will_do (grand); one milestone effect; active; allowed; available, ai_will_do (subdoctrine).
    assert_eq!(report.count(&IssueKind::Deferred), 7);
    assert_eq!(report.count(&IssueKind::UnknownKey), 0);
}

/// The real installation (skipped without `HOI4_DIR`).
#[test]
fn real_country_leader_traits_and_doctrines() {
    let Ok(dir) = std::env::var("HOI4_DIR") else { return };
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let (db, report) = Database::load(&vfs, &LoadOptions::default());
    assert!(db.country_leader_traits.len() > 2_000, "{}", db.country_leader_traits.len());
    assert!(db.country_leader_traits.get("captain_of_industry").is_some());
    let d = &db.doctrines;
    assert_eq!(d.folders.len(), 4);
    assert!(d.grand_doctrines.len() >= 12 && d.subdoctrines.len() > 90 && d.tracks.len() >= 14);
    let mw = d.grand_doctrine("new_mobile_warfare").expect("new_mobile_warfare");
    assert_eq!(mw.folder.as_deref(), Some("land"));
    assert_eq!(mw.milestones.len(), mw.tracks.len());
    assert!(d.subdoctrine("mobile_infantry").is_some_and(|s| !s.rewards.is_empty()));
    for prefix in ["country_leader_trait", "doctrine", "grand_doctrine", "subdoctrine"] {
        let bad: Vec<String> = report
            .issues
            .iter()
            .filter(|i| i.path.starts_with(prefix) && (i.kind == IssueKind::UnknownKey || i.kind.is_error()))
            // Five traits of `00_traits.txt` are defined twice (three identically); the first is kept.
            .filter(|i| !(i.path == "country_leader_trait" && i.kind == IssueKind::Redefinition("first")))
            .map(|i| i.to_string())
            .collect();
        assert!(bad.is_empty(), "{prefix}: {bad:#?}");
    }
    let redefined = report
        .issues
        .iter()
        .filter(|i| i.path == "country_leader_trait" && i.kind == IssueKind::Redefinition("first"))
        .count();
    assert_eq!(redefined, 5);
}
