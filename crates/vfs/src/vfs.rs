//! The merged view over all layers.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::layer::Layer;
use crate::path::{normalize, parent};

#[derive(Debug)]
pub enum VfsError {
    /// The file is not present in any layer.
    NotFound(String),
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VfsError::NotFound(p) => write!(f, "file not found in any layer: {p}"),
            VfsError::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for VfsError {}

/// A file visible through the VFS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Relative path with its original capitalisation and `/` separators.
    pub path: String,
    /// Where it really is on disk.
    pub real_path: PathBuf,
    /// Index of the providing layer (see [`Vfs::layers`]).
    pub layer: usize,
}

/// Merged, read-only view over a stack of layers.
#[derive(Debug, Clone)]
pub struct Vfs {
    layers: Vec<Layer>,
    files: BTreeMap<String, FileEntry>,
}

impl Vfs {
    /// Scans every layer (lowest priority first) and builds the index. Unreadable directories are skipped; a layer
    /// whose root does not exist contributes nothing.
    pub fn new(layers: Vec<Layer>) -> Vfs {
        let mut files: BTreeMap<String, FileEntry> = BTreeMap::new();
        for (index, layer) in layers.iter().enumerate() {
            for dir in &layer.replace_paths {
                let prefix = normalize(dir);
                if prefix.is_empty() {
                    continue;
                }
                let dir_prefix = format!("{prefix}/");
                files.retain(|key, _| !key.starts_with(&dir_prefix));
            }
            let skipped: Vec<String> = layer.skip_dirs.iter().map(|d| normalize(d)).collect();
            let mut stack = vec![(layer.root.clone(), String::new())];
            while let Some((dir, rel)) = stack.pop() {
                let Ok(read) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in read.filter_map(|e| e.ok()) {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let rel_path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                    let path = entry.path();
                    let Ok(kind) = entry.file_type() else {
                        continue;
                    };
                    if kind.is_dir() {
                        if !skipped.contains(&normalize(&rel_path)) {
                            stack.push((path, rel_path));
                        }
                    } else if kind.is_file() {
                        files.insert(normalize(&rel_path), FileEntry { path: rel_path, real_path: path, layer: index });
                    }
                }
            }
        }
        Vfs { layers, files }
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Number of distinct files visible.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn get(&self, path: &str) -> Option<&FileEntry> {
        self.files.get(&normalize(path))
    }

    pub fn exists(&self, path: &str) -> bool {
        self.get(path).is_some()
    }

    /// The layer that provides `path`.
    pub fn layer_of(&self, path: &str) -> Option<&Layer> {
        self.get(path).and_then(|e| self.layers.get(e.layer))
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, VfsError> {
        let entry = self.get(path).ok_or_else(|| VfsError::NotFound(path.to_string()))?;
        std::fs::read(&entry.real_path).map_err(|source| VfsError::Io { path: entry.real_path.clone(), source })
    }

    /// Reads a text file, decoding it the way game files are decoded (UTF-8, BOM stripped, lossy).
    pub fn read_text(&self, path: &str) -> Result<String, VfsError> {
        Ok(clausewitz::decode(&self.read(path)?).into_owned())
    }

    /// Files directly inside `dir`, sorted by normalised path.
    pub fn list(&self, dir: &str) -> Vec<&FileEntry> {
        let key = normalize(dir);
        self.files.iter().filter(|(k, _)| parent(k) == key).map(|(_, e)| e).collect()
    }

    /// Files under `dir` (any depth) with the given extension (without the dot, case-insensitive), sorted.
    pub fn list_recursive(&self, dir: &str, extension: &str) -> Vec<&FileEntry> {
        let key = normalize(dir);
        let prefix = if key.is_empty() { String::new() } else { format!("{key}/") };
        let wanted = extension.to_lowercase();
        self.files
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .filter(|(k, _)| Path::new(k.as_str()).extension().is_some_and(|e| e.to_string_lossy() == wanted))
            .map(|(_, e)| e)
            .collect()
    }
}
