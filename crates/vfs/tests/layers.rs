use std::path::{Path, PathBuf};

use vfs::{Layer, Vfs, VfsError, discover_game_layers, normalize, parse_descriptor};

/// A scratch directory that is removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("clausefree_vfs_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch(dir)
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        std::fs::write(path, content).expect("write file");
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
fn later_layers_override_earlier_files() {
    let base = Scratch::new("base");
    let dlc = Scratch::new("dlc");
    base.write("common/a.txt", "base a");
    base.write("common/b.txt", "base b");
    dlc.write("common/b.txt", "dlc b");
    dlc.write("common/c.txt", "dlc c");
    let vfs = Vfs::new(vec![Layer::new("base", base.path()), Layer::new("dlc", dlc.path())]);
    assert_eq!(vfs.read_text("common/a.txt").expect("a"), "base a");
    assert_eq!(vfs.read_text("common/b.txt").expect("b"), "dlc b");
    assert_eq!(vfs.read_text("common/c.txt").expect("c"), "dlc c");
    assert_eq!(vfs.layer_of("common/b.txt").map(|l| l.name.as_str()), Some("dlc"));
    assert_eq!(vfs.layer_of("common/a.txt").map(|l| l.name.as_str()), Some("base"));
    assert_eq!(vfs.len(), 3);
}

#[test]
fn replace_path_drops_everything_below_it() {
    let base = Scratch::new("rp_base");
    let modl = Scratch::new("rp_mod");
    base.write("history/states/1.txt", "base 1");
    base.write("history/states/sub/2.txt", "base 2");
    base.write("history/countries/GER.txt", "base ger");
    modl.write("history/states/9.txt", "mod 9");
    let vfs = Vfs::new(vec![
        Layer::new("base", base.path()),
        Layer::new("mod", modl.path()).with_replace_paths(["history/states".to_string()]),
    ]);
    assert!(!vfs.exists("history/states/1.txt"));
    assert!(!vfs.exists("history/states/sub/2.txt"), "nested files are dropped too");
    assert!(vfs.exists("history/states/9.txt"));
    assert!(vfs.exists("history/countries/GER.txt"), "other directories are untouched");
    // A replace_path never removes the layer's own files or later layers' files.
    let later = Scratch::new("rp_later");
    later.write("history/states/10.txt", "later 10");
    let vfs = Vfs::new(vec![
        Layer::new("base", base.path()),
        Layer::new("mod", modl.path()).with_replace_paths(["history/states/".to_string()]),
        Layer::new("later", later.path()),
    ]);
    assert!(vfs.exists("history/states/9.txt") && vfs.exists("history/states/10.txt"));
}

#[test]
fn lookups_ignore_case_and_separators() {
    let base = Scratch::new("case");
    base.write("Common/Units/Infantry.TXT", "x");
    let vfs = Vfs::new(vec![Layer::new("base", base.path())]);
    assert!(vfs.exists("common/units/infantry.txt"));
    assert!(vfs.exists("COMMON\\UNITS\\INFANTRY.TXT"));
    assert!(vfs.exists("./common//units/infantry.txt"));
    assert_eq!(vfs.get("common/units/infantry.txt").map(|e| e.path.as_str()), Some("Common/Units/Infantry.TXT"));
    assert_eq!(normalize("/A\\B//C/"), "a/b/c");
    assert!(!vfs.exists("../common/units/infantry.txt"), "no escaping through ..");
}

#[test]
fn listing_directories_and_extensions() {
    let base = Scratch::new("list");
    base.write("common/units/a.txt", "");
    base.write("common/units/b.TXT", "");
    base.write("common/units/c.gfx", "");
    base.write("common/units/deep/d.txt", "");
    base.write("common/other.txt", "");
    let vfs = Vfs::new(vec![Layer::new("base", base.path())]);
    let direct: Vec<&str> = vfs.list("common/units").iter().map(|e| e.path.as_str()).collect();
    assert_eq!(direct.len(), 3, "{direct:?}");
    let txt: Vec<String> = vfs.list_recursive("common/units", "txt").iter().map(|e| e.path.to_lowercase()).collect();
    assert_eq!(txt, vec!["common/units/a.txt", "common/units/b.txt", "common/units/deep/d.txt"]);
    assert_eq!(vfs.list_recursive("", "txt").len(), 4);
    assert!(vfs.list("nope").is_empty());
}

#[test]
fn errors_are_values_not_panics() {
    let vfs = Vfs::new(vec![Layer::new("ghost", "Z:/definitely/not/here")]);
    assert!(vfs.is_empty());
    assert!(matches!(vfs.read("a/b.txt"), Err(VfsError::NotFound(_))));
    assert!(vfs.read_text("a/b.txt").is_err());
    assert!(vfs.list("a").is_empty());
}

#[test]
fn skip_dirs_keep_dlc_out_of_the_base_layer() {
    let game = Scratch::new("game");
    game.write("common/a.txt", "base");
    game.write("dlc/dlc001_x/common/a.txt", "dlc one");
    game.write("dlc/dlc001_x/common/only_dlc.txt", "dlc only");
    game.write("integrated_dlc/dlc018_y/common/z.txt", "integrated");
    let layers = discover_game_layers(game.path());
    let names: Vec<&str> = layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["base", "integrated_dlc/dlc018_y", "dlc/dlc001_x"]);
    let vfs = Vfs::new(layers);
    assert_eq!(vfs.read_text("common/a.txt").expect("a"), "dlc one", "DLC overrides the base file");
    assert!(vfs.exists("common/only_dlc.txt") && vfs.exists("common/z.txt"));
    assert!(!vfs.exists("dlc/dlc001_x/common/a.txt"), "DLC folders are not visible as paths");
    assert!(!vfs.exists("integrated_dlc/dlc018_y/common/z.txt"));
}

#[test]
fn mod_descriptors_are_parsed() {
    let d = parse_descriptor(
        "name = \"My Mod\"\npath = \"mod/my_mod\"\nsupported_version = \"1.16.*\"\nreplace_path = \"common/ideas\"\nreplace_path=\"history/states\"\ndependencies = { \"Base Mod\" \"Other\" }\ntags = { \"Gameplay\" }\n",
    );
    assert_eq!(d.name, "My Mod");
    assert_eq!(d.path.as_deref(), Some("mod/my_mod"));
    assert_eq!(d.replace_paths, vec!["common/ideas", "history/states"]);
    assert_eq!(d.dependencies, vec!["Base Mod", "Other"]);
    assert_eq!(d.supported_version.as_deref(), Some("1.16.*"));
    // Garbage in, empty descriptor out.
    assert_eq!(parse_descriptor("}{ = = \""), Default::default());
}

#[test]
fn layer_from_a_mod_descriptor_file() {
    let user = Scratch::new("user");
    user.write("mod/foo.mod", "name = \"Foo\"\npath = \"mod/foo\"\nreplace_path = \"common/units\"\n");
    user.write("mod/foo/common/units/x.txt", "x");
    let layer = Layer::from_mod_descriptor(&user.path().join("mod/foo.mod"), user.path()).expect("layer");
    assert_eq!(layer.name, "Foo");
    assert_eq!(layer.root, user.path().join("mod/foo"));
    assert_eq!(layer.replace_paths, vec!["common/units"]);
    // No `path`: the folder next to the descriptor is used. A missing descriptor gives None.
    user.write("mod/bar.mod", "name = \"Bar\"\n");
    let bar = Layer::from_mod_descriptor(&user.path().join("mod/bar.mod"), user.path()).expect("layer");
    assert_eq!(bar.root, user.path().join("mod/bar"));
    assert!(Layer::from_mod_descriptor(&user.path().join("mod/none.mod"), user.path()).is_none());
}

/// The real installation (skipped without `HOI4_DIR`): base files and DLC overlays are both visible.
#[test]
fn real_installation_is_layered() {
    let Ok(dir) = std::env::var("HOI4_DIR") else {
        return;
    };
    let game = Path::new(&dir);
    let layers = discover_game_layers(game);
    assert!(layers.len() >= 2 && layers[0].name == "base");
    let vfs = Vfs::new(layers);
    assert!(vfs.len() > 10_000, "only {} files", vfs.len());
    assert_eq!(vfs.layer_of("common/units/infantry.txt").map(|l| l.name.as_str()), Some("base"));
    assert!(
        vfs.layers()
            .iter()
            .skip(1)
            .any(|l| vfs.list_recursive("", "txt").iter().any(|e| vfs.layers()[e.layer].name == l.name)),
        "no DLC layer contributed any script file"
    );
    assert!(vfs.read_text("common/units/infantry.txt").is_ok_and(|t| t.contains("sub_units")));
    assert!(!vfs.exists("dlc/dlc001_german_historical_portraits/dlc001.dlc"));
}
