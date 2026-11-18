//! Layers: one directory tree plus its `replace_path` rules.

use std::path::{Path, PathBuf};

use crate::descriptor::parse_descriptor;

/// One source of game files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    /// Display name (for diagnostics and "which layer provides this file" queries).
    pub name: String,
    pub root: PathBuf,
    /// Directories (relative, any separator) whose content from all *earlier* layers is ignored.
    pub replace_paths: Vec<String>,
    /// Directories (relative) that are not scanned at all. The base game uses this to leave out the `dlc` and
    /// `integrated_dlc` folders, which are layers of their own.
    pub skip_dirs: Vec<String>,
}

impl Layer {
    pub fn new(name: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self { name: name.into(), root: root.into(), replace_paths: Vec::new(), skip_dirs: Vec::new() }
    }

    pub fn with_skip_dirs(mut self, dirs: impl IntoIterator<Item = String>) -> Self {
        self.skip_dirs = dirs.into_iter().collect();
        self
    }

    pub fn with_replace_paths(mut self, paths: impl IntoIterator<Item = String>) -> Self {
        self.replace_paths = paths.into_iter().collect();
        self
    }

    /// Builds a layer from a mod descriptor file. `path` is resolved against `anchor` when relative (a mod's `path`
    /// is relative to the user data directory, not to the descriptor file). Without a `path`, the folder next to the
    /// descriptor named like it (`foo.mod` -> `foo/`) is used. Returns `None` if the descriptor cannot be read.
    pub fn from_mod_descriptor(descriptor_file: &Path, anchor: &Path) -> Option<Layer> {
        let text = clausewitz::decode(&std::fs::read(descriptor_file).ok()?).into_owned();
        let d = parse_descriptor(&text);
        let root = match d.path.as_deref() {
            Some(p) if Path::new(p).is_absolute() => PathBuf::from(p),
            Some(p) => anchor.join(p),
            None => descriptor_file.with_extension(""),
        };
        let name = if d.name.is_empty() {
            descriptor_file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            d.name
        };
        Some(Layer { name, root, replace_paths: d.replace_paths, skip_dirs: Vec::new() })
    }
}

/// Layers of an installation, lowest priority first: the base game, then `integrated_dlc/*`, then `dlc/*`, each
/// group sorted by folder name. **The relative order of DLC is an assumption** (to be verified against the original,
/// see docs/CANON.md); callers may reorder the result.
pub fn discover_game_layers(game_dir: &Path) -> Vec<Layer> {
    let mut layers =
        vec![Layer::new("base", game_dir).with_skip_dirs(["dlc".to_string(), "integrated_dlc".to_string()])];
    for group in ["integrated_dlc", "dlc"] {
        let Ok(read) = std::fs::read_dir(game_dir.join(group)) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
        dirs.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
        for dir in dirs {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            layers.push(Layer::new(format!("{group}/{name}"), dir));
        }
    }
    layers
}
