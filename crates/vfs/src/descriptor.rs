//! `.mod` / `.dlc` descriptors. Both are Paradox script files with a handful of keys.

/// The parts of a descriptor the engine cares about.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Descriptor {
    pub name: String,
    /// `path = "..."`: where the content lives (relative to the descriptor's anchor, or absolute).
    pub path: Option<String>,
    /// `replace_path = "..."` entries, in order.
    pub replace_paths: Vec<String>,
    /// `dependencies = { "Other Mod" }`.
    pub dependencies: Vec<String>,
    pub supported_version: Option<String>,
}

/// Parses descriptor text. Unknown keys are ignored; malformed text yields whatever could be read.
pub fn parse_descriptor(text: &str) -> Descriptor {
    let parsed = clausewitz::parse(text);
    let mut d = Descriptor::default();
    for entry in parsed.root.entries() {
        let text_of = |e: &clausewitz::Entry<'_>| e.value.as_scalar().map(|s| s.unescaped().into_owned());
        match entry.key.text {
            "name" => d.name = text_of(entry).unwrap_or_default(),
            "path" => d.path = text_of(entry),
            "replace_path" => d.replace_paths.extend(text_of(entry)),
            "supported_version" => d.supported_version = text_of(entry),
            "dependencies" => {
                if let Some(block) = entry.value.as_block() {
                    d.dependencies
                        .extend(block.values().filter_map(|v| v.as_scalar()).map(|s| s.unescaped().into_owned()));
                }
            }
            _ => {}
        }
    }
    d
}
