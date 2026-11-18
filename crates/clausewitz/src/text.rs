//! Turning raw file bytes into text.

use std::borrow::Cow;

/// Decodes a game file. Files are UTF-8 (sometimes with a byte order mark); invalid bytes become `U+FFFD` instead of
/// failing, so a single bad character never makes a whole file unreadable.
pub fn decode(bytes: &[u8]) -> Cow<'_, str> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes)
}
