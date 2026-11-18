//! Layered virtual file system for game data.
//!
//! Game content is never read from the real file system directly. It is read through a [`Vfs`] that stacks
//! *layers* in priority order (base game, then DLC, then mods). A file in a later layer replaces the file with
//! the same relative path in an earlier one, and a layer can declare `replace_path` directories whose content
//! from all earlier layers is dropped, exactly like the original's mod system.
//!
//! Paths inside the VFS are relative, use `/` and are compared case-insensitively.

mod descriptor;
mod layer;
mod path;
mod vfs;

pub use descriptor::{Descriptor, parse_descriptor};
pub use layer::{Layer, discover_game_layers};
pub use path::normalize;
pub use vfs::{FileEntry, Vfs, VfsError};
