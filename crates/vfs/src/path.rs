//! Path normalisation shared by the whole VFS.

/// Canonical key of a relative path: forward slashes, lower case, no leading `./` or `/`, no empty segments and no
/// trailing slash. `.` segments are dropped; `..` is kept as an ordinary (never-matching) segment, so a path cannot
/// escape its layer.
pub fn normalize(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for segment in path.split(['/', '\\']) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&segment.to_lowercase());
    }
    out
}

/// Parent directory key of a normalised path (empty for top-level entries).
pub(crate) fn parent(key: &str) -> &str {
    key.rfind('/').map_or("", |i| &key[..i])
}
