//! Small value types shared by the definitions.

use std::fmt;

use clausewitz::Value;
use common::Fixed;

/// A list of `name = number` pairs, kept in file order.
pub type NumberList = Vec<(String, Fixed)>;

/// Country tag: exactly three ASCII letters or digits (`GER`, `D01`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tag([u8; 3]);

impl Tag {
    /// Accepts upper-case letters and digits only, as used by every tag in the game files.
    pub fn parse(text: &str) -> Option<Tag> {
        let bytes = text.as_bytes();
        if bytes.len() != 3 || !bytes.iter().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) {
            return None;
        }
        Some(Tag([bytes[0], bytes[1], bytes[2]]))
    }

    pub fn as_str(&self) -> &str {
        // Construction guarantees ASCII.
        std::str::from_utf8(&self.0).unwrap_or("???")
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tag({})", self.as_str())
    }
}

/// A colour as written in the files. Conversion to screen colours belongs to the renderer, because the original's
/// HSV conversion is not documented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// `{ r g b }` or `rgb { r g b }`, components 0..=255.
    Rgb([u8; 3]),
    /// `hsv { h s v }`, components as written (normally 0..=1).
    Hsv([Fixed; 3]),
}

impl Color {
    /// Reads `{ 1 2 3 }`, `rgb { 1 2 3 }`, `RGB { ... }` or `hsv { 0.1 0.2 0.3 }`. `None` if malformed.
    pub fn from_value(value: &Value<'_>) -> Option<Color> {
        let (kind, block) = match value {
            Value::Block(b) => ("rgb".to_string(), b),
            Value::Tagged(tag, b) => (tag.text.to_ascii_lowercase(), b),
            _ => return None,
        };
        if block.entries().next().is_some() {
            return None;
        }
        let parts: Vec<&str> = block.values().filter_map(|v| v.as_str()).collect();
        if parts.len() != 3 || block.values().count() != 3 {
            return None;
        }
        match kind.as_str() {
            "rgb" => {
                let mut c = [0u8; 3];
                for (slot, text) in c.iter_mut().zip(&parts) {
                    *slot = text.parse::<u8>().ok()?;
                }
                Some(Color::Rgb(c))
            }
            "hsv" => {
                let mut c = [Fixed::ZERO; 3];
                for (slot, text) in c.iter_mut().zip(&parts) {
                    *slot = Fixed::parse(text).ok()?;
                }
                Some(Color::Hsv(c))
            }
            _ => None,
        }
    }
}

/// Game date with an optional hour, as used in history files (`1936.1.1`, `1936.1.1.12`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
}

impl Date {
    /// Parses `Y.M.D` or `Y.M.D.H`. Range checks are loose on purpose (the original accepts odd dates in files);
    /// only shape and basic bounds are enforced.
    pub fn parse(text: &str) -> Option<Date> {
        let mut parts = text.split('.');
        let year = parts.next()?.parse::<i32>().ok()?;
        let month = parts.next()?.parse::<u8>().ok().filter(|m| (1..=12).contains(m))?;
        let day = parts.next()?.parse::<u8>().ok().filter(|d| (1..=31).contains(d))?;
        let hour = match parts.next() {
            Some(h) => h.parse::<u8>().ok().filter(|h| *h <= 24)?,
            None => 0,
        };
        if parts.next().is_some() {
            return None;
        }
        Some(Date { year, month, day, hour })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        assert_eq!(Tag::parse("GER").map(|t| t.to_string()), Some("GER".to_string()));
        assert!(Tag::parse("D01").is_some());
        for bad in ["ger", "GE", "GERM", "G-R", "", "ÄBC"] {
            assert!(Tag::parse(bad).is_none(), "{bad}");
        }
        assert!(Tag::parse("ENG") < Tag::parse("GER"));
    }

    #[test]
    fn colors() {
        let parse = |text: &str| {
            let p = clausewitz::parse(text);
            p.root.get("c").and_then(|e| Color::from_value(&e.value))
        };
        assert_eq!(parse("c = { 106 119 89 }"), Some(Color::Rgb([106, 119, 89])));
        assert_eq!(parse("c = rgb { 1 2 3 }"), Some(Color::Rgb([1, 2, 3])));
        assert_eq!(parse("c = RGB { 1 2 3 }"), Some(Color::Rgb([1, 2, 3])));
        let hsv = parse("c = HSV { 0.1 0.15 0.4 }");
        assert!(
            matches!(hsv, Some(Color::Hsv([h, _, v])) if h == Fixed::parse("0.1").unwrap_or_default() && v == Fixed::parse("0.4").unwrap_or_default())
        );
        for bad in
            ["c = { 1 2 }", "c = { 1 2 3 4 }", "c = { 1 2 300 }", "c = { a = 1 b c }", "c = cmyk { 1 2 3 }", "c = 5"]
        {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn dates() {
        assert_eq!(Date::parse("1936.1.1"), Some(Date { year: 1936, month: 1, day: 1, hour: 0 }));
        assert_eq!(Date::parse("1936.11.9.1").map(|d| d.hour), Some(1));
        assert!(Date::parse("1939.1.1") > Date::parse("1936.12.31"));
        for bad in ["1936", "1936.1", "1936.13.1", "1936.1.32", "1936.1.1.1.1", "x.1.1", "id"] {
            assert!(Date::parse(bad).is_none(), "{bad}");
        }
    }
}
