use std::path::{Path, PathBuf};

use common::Fixed;
use data::defines::{self, Defines};
use data::lua::{self, LuaEnv, LuaValue};
use data::report::{IssueKind, Report};
use data::source::Source;

fn fixed(text: &str) -> Fixed {
    Fixed::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

fn run(text: &str) -> (Defines, Vec<lua::LuaIssue>) {
    let mut d = Defines::default();
    let issues = lua::run(text, &mut d.env);
    (d, issues)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("clausefree_defines_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(path, content).expect("write");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const BASE: &str = r#"
NDefines = {

NGame = {
	START_DATE = "1936.1.1.12",
	MAP_SCALE_PIXEL_TO_KM = 7.114,					-- Yes, we did the math
	GAME_SPEED_SECONDS = { 2.0, 0.5, 0.2, 0.1, 0.0 }, -- game speeds
	NAME = 'single quoted',
	ESCAPED = "say \"hi\"",
	FLAG = true,
	OFF = false,
	NOTHING = nil,
	NEGATIVE = -0.75,
	HOURS = 7 * 24,
	MIXED = 2 + 3 * 4,
	GROUPED = (2 + 3) * 4,
	THIRD = 1 / 3,
	NESTED = { a = 1, ["quoted key"] = 2, [3] = 4; "list" },
},

NMilitary = {
	--[[ a long
	     comment with } and = inside ]]
	BASE_ORG = 60,
	--[==[ another ]] still comment ]==]
	LEVELS = { { 1, 2 }, { 3 } },
},
}
"#;

#[test]
fn values_tables_lists_and_comments() {
    let (d, issues) = run(BASE);
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(d.string("NDefines.NGame.START_DATE"), Some("1936.1.1.12"));
    assert_eq!(d.number("NDefines.NGame.MAP_SCALE_PIXEL_TO_KM"), Some(fixed("7.114")));
    let speeds = d.get("NDefines.NGame.GAME_SPEED_SECONDS").and_then(LuaValue::as_table).expect("list");
    assert_eq!(speeds.list.len(), 5);
    assert_eq!(speeds.list[1].as_number(), Some(fixed("0.5")));
    assert_eq!(d.string("NDefines.NGame.NAME"), Some("single quoted"));
    assert_eq!(d.string("NDefines.NGame.ESCAPED"), Some("say \"hi\""));
    assert_eq!((d.bool("NDefines.NGame.FLAG"), d.bool("NDefines.NGame.OFF")), (Some(true), Some(false)));
    assert_eq!(d.get("NDefines.NGame.NOTHING"), Some(&LuaValue::Nil));
    assert_eq!(d.number("NDefines.NGame.NEGATIVE"), Some(fixed("-0.75")));
    let nested = d.section("NDefines.NGame.NESTED").expect("nested");
    assert_eq!(nested.get("quoted key").and_then(LuaValue::as_number), Some(fixed("2")));
    assert_eq!(nested.get("3").and_then(LuaValue::as_number), Some(fixed("4")));
    assert_eq!(nested.list, vec![LuaValue::String("list".to_string())]);
    assert_eq!(d.define("NMilitary", "BASE_ORG").and_then(LuaValue::as_number), Some(fixed("60")));
    let levels = d.section("NDefines.NMilitary.LEVELS").expect("levels");
    assert_eq!(levels.list.len(), 2);
    assert!(d.get("NDefines.NGame.MISSING").is_none() && d.get("Nope.X").is_none());
}

#[test]
fn arithmetic_follows_lua_precedence() {
    let (d, _) = run(BASE);
    assert_eq!(d.number("NDefines.NGame.HOURS"), Some(fixed("168")));
    assert_eq!(d.number("NDefines.NGame.MIXED"), Some(fixed("14")));
    assert_eq!(d.number("NDefines.NGame.GROUPED"), Some(fixed("20")));
    assert_eq!(d.number("NDefines.NGame.THIRD"), Some(fixed("0.33333")), "fixed point, rounded toward zero");
    let (e, issues) = run("A = { X = 1 / 0, Y = 2 - -3, Z = -(1 + 1) }");
    assert_eq!(issues.len(), 1, "division by zero is reported");
    assert!(e.get("A.X").is_none());
    assert_eq!((e.number("A.Y"), e.number("A.Z")), (Some(fixed("5")), Some(fixed("-2"))));
}

#[test]
fn mod_style_overrides_and_the_merge_loop() {
    let text = format!(
        "{BASE}\nNDefines.NGame.START_DATE = \"1937.1.1.12\"\nNDefines[\"NMilitary\"].BASE_ORG = 70\n\
         NDefines_Graphics = {{ NMapMode = {{ ALPHA = 0.5 }} }}\n\
         for k,v in pairs( NDefines_Graphics ) do NDefines[k] = v end\n\
         NDefines.NNope.X = 1\n"
    );
    let (d, issues) = run(&text);
    assert_eq!(d.string("NDefines.NGame.START_DATE"), Some("1937.1.1.12"));
    assert_eq!(d.number("NDefines.NMilitary.BASE_ORG"), Some(fixed("70")));
    assert_eq!(d.number("NDefines.NMapMode.ALPHA"), Some(fixed("0.5")), "merged by the loop");
    assert_eq!(issues.len(), 1, "overriding an undefined section is reported: {issues:?}");
    assert!(!issues[0].unsupported);
}

#[test]
fn unsupported_lua_is_reported_and_skipped() {
    let (d, issues) = run(
        "local x = 5\nA = { B = 1, C = some_variable, D = 2 }\nfunction f() return 1 end\nE = 3\nprint(\"hi\")\nF = 4\n",
    );
    assert!(issues.iter().any(|i| i.unsupported && i.message.contains("local")));
    assert!(issues.iter().any(|i| i.unsupported && i.message.contains("some_variable")));
    assert!(issues.iter().any(|i| i.unsupported && i.message.contains("function")));
    // The rest is still read: the bad element alone is skipped.
    assert_eq!((d.number("A.B"), d.number("A.D"), d.get("A.C")), (Some(fixed("1")), Some(fixed("2")), None));
    assert_eq!((d.number("E"), d.number("F")), (Some(fixed("3")), Some(fixed("4"))));
}

#[test]
fn broken_text_never_panics() {
    for text in [
        "",
        "A",
        "A =",
        "A = {",
        "A = { B = }",
        "A = \"open",
        "--[[ never closed",
        "}}}} = = ,,, ((((",
        "A = 99999999999999999",
        "A = 1e5",
        "for k,v in pairs( X ) do",
        "A = -",
    ] {
        let mut env = LuaEnv::default();
        let issues = lua::run(text, &mut env);
        assert!(text.is_empty() || !issues.is_empty(), "{text:?} should report a problem");
    }
    let deep = format!("A = {}{}", "{".repeat(10_000), "}".repeat(10_000));
    assert!(!lua::run(&deep, &mut LuaEnv::default()).is_empty());
    let deep_math = format!("A = {}1{}", "(".repeat(10_000), ")".repeat(10_000));
    assert!(!lua::run(&deep_math, &mut LuaEnv::default()).is_empty());
}

#[test]
fn files_run_in_evaluation_order_across_layers() {
    let base = Scratch::new("base");
    let modl = Scratch::new("mod");
    base.write("common/defines/00_defines.lua", "NDefines = { NGame = { START_DATE = \"1936.1.1.12\", END = 1 } }");
    modl.write(
        "common/defines/zz_mod_defines.lua",
        "NDefines.NGame.START_DATE = \"1940.1.1.12\"\nNDefines.NGame.LATE = 2",
    );
    let vfs = vfs::Vfs::new(vec![vfs::Layer::new("base", &base.0), vfs::Layer::new("mod", &modl.0)]);
    let mut report = Report::default();
    let d = defines::load(&vfs, &mut report);
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    assert_eq!(d.string("NDefines.NGame.START_DATE"), Some("1940.1.1.12"), "the mod file runs later");
    assert_eq!(d.number("NDefines.NGame.END"), Some(fixed("1")));
    // A mod file with the base file's name replaces it entirely (VFS), so its own table is the only one.
    modl.write("common/defines/00_defines.lua", "NDefines = { NGame = { START_DATE = \"1950.1.1.1\" } }");
    let vfs = vfs::Vfs::new(vec![vfs::Layer::new("base", &base.0), vfs::Layer::new("mod", &modl.0)]);
    let d = defines::load(&vfs, &mut report);
    assert_eq!(d.number("NDefines.NGame.END"), None);
    assert_eq!(d.string("NDefines.NGame.START_DATE"), Some("1940.1.1.12"), "zz_ still overrides after 00_");
    // Problems carry file and line.
    let mut report = Report::default();
    let mut d = Defines::default();
    defines::run_file(&Source::from_text("common/defines/x.lua", "A = 1\n\nB = {".to_string()), &mut report, &mut d);
    assert_eq!(report.issues.len(), 1);
    assert_eq!((report.issues[0].file.as_str(), report.issues[0].line), ("common/defines/x.lua", 3));
    assert!(matches!(report.issues[0].kind, IssueKind::InvalidValue(_)));
}

/// The real installation (skipped without `HOI4_DIR`).
#[test]
fn real_defines_load_completely() {
    let Ok(dir) = std::env::var("HOI4_DIR") else { return };
    let vfs = vfs::Vfs::new(vfs::discover_game_layers(Path::new(&dir)));
    let mut report = Report::default();
    let d = defines::load(&vfs, &mut report);
    assert!(report.issues.is_empty(), "{:?}", report.issues.iter().take(3).collect::<Vec<_>>());
    assert_eq!(d.string("NDefines.NGame.START_DATE"), Some("1936.1.1.12"));
    assert_eq!(d.number("NDefines.NGame.MAP_SCALE_PIXEL_TO_KM"), Some(fixed("7.114")));
    assert_eq!(d.section("NDefines.NGame.GAME_SPEED_SECONDS").map(|t| t.list.len()), Some(5));
    let ndefines = d.section("NDefines").expect("NDefines");
    assert!(ndefines.fields.len() > 25, "{} sections", ndefines.fields.len());
    // Written as `7 * 24` in the file.
    let patrols = ndefines
        .fields
        .iter()
        .find_map(|(_, s)| s.as_table()?.get("MIN_HOURS_TO_SHUFFLE_NEWLY_ASSIGNED_PATROLS"))
        .and_then(LuaValue::as_number);
    assert_eq!(patrols, Some(fixed("168")));
    // Graphics and career profile tables are merged into NDefines by the loops at the end of their files.
    assert!(d.section("NDefines.NMapMode").is_some() && d.section("NDefines.NCareerProfile").is_some());
}
