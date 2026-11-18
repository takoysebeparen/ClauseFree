//! Syntax tree for Paradox script.
//!
//! The tree borrows from the source text (no copies) and keeps everything needed to reproduce the file's structure:
//! key order, duplicate keys, the operator used, and byte spans for error messages.

use std::borrow::Cow;

/// Byte range in the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        // Files larger than 4 GiB are not game files; saturate instead of panicking.
        Self { start: u32::try_from(start).unwrap_or(u32::MAX), end: u32::try_from(end).unwrap_or(u32::MAX) }
    }

    pub fn len(&self) -> usize {
        (self.end - self.start) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }

    /// Smallest span covering both.
    pub fn to(self, other: Span) -> Span {
        Span { start: self.start.min(other.start), end: self.end.max(other.end) }
    }
}

/// Comparison / assignment operator between a key and its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `=`
    Eq,
    /// `==`
    EqEq,
    /// `!=`
    NotEq,
    /// `<`
    Lt,
    /// `>`
    Gt,
    /// `<=`
    Le,
    /// `>=`
    Ge,
    /// `?=` (assign if the value exists)
    OptionalEq,
}

impl Operator {
    pub fn as_str(self) -> &'static str {
        match self {
            Operator::Eq => "=",
            Operator::EqEq => "==",
            Operator::NotEq => "!=",
            Operator::Lt => "<",
            Operator::Gt => ">",
            Operator::Le => "<=",
            Operator::Ge => ">=",
            Operator::OptionalEq => "?=",
        }
    }
}

/// A bare word, number or quoted string. The text is exactly as written (escape sequences are not processed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scalar<'a> {
    /// Text without the surrounding quotes for quoted strings.
    pub text: &'a str,
    pub quoted: bool,
    pub span: Span,
}

impl<'a> Scalar<'a> {
    /// The text with `\"` and `\\` escapes resolved (borrowed when there is nothing to resolve).
    pub fn unescaped(&self) -> Cow<'a, str> {
        if !self.quoted || !self.text.contains('\\') {
            return Cow::Borrowed(self.text);
        }
        let mut out = String::with_capacity(self.text.len());
        let mut chars = self.text.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('n') => out.push('\n'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        Cow::Owned(out)
    }

    /// Integer value of an unquoted scalar (`-5`, `12`). `None` for anything else, including decimals.
    pub fn as_i64(&self) -> Option<i64> {
        if self.quoted { None } else { self.text.parse().ok() }
    }

    /// Paradox boolean: `yes` / `no` (unquoted).
    pub fn as_bool(&self) -> Option<bool> {
        if self.quoted {
            return None;
        }
        match self.text {
            "yes" => Some(true),
            "no" => Some(false),
            _ => None,
        }
    }
}

/// Anything that can stand on the right-hand side of an operator, or on its own inside a list.
#[derive(Debug, Clone, PartialEq)]
pub enum Value<'a> {
    Scalar(Scalar<'a>),
    Block(Block<'a>),
    /// A tag directly followed by a block, e.g. `rgb { 255 0 0 }`.
    Tagged(Scalar<'a>, Block<'a>),
    /// The value was missing in malformed input (a diagnostic is reported alongside).
    Missing(Span),
}

impl<'a> Value<'a> {
    pub fn span(&self) -> Span {
        match self {
            Value::Scalar(s) => s.span,
            Value::Block(b) => b.span,
            Value::Tagged(tag, b) => tag.span.to(b.span),
            Value::Missing(span) => *span,
        }
    }

    pub fn as_scalar(&self) -> Option<&Scalar<'a>> {
        match self {
            Value::Scalar(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_block(&self) -> Option<&Block<'a>> {
        match self {
            Value::Block(b) | Value::Tagged(_, b) => Some(b),
            _ => None,
        }
    }

    /// Text of a scalar value.
    pub fn as_str(&self) -> Option<&'a str> {
        self.as_scalar().map(|s| s.text)
    }
}

/// `key operator value`.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry<'a> {
    pub key: Scalar<'a>,
    pub op: Operator,
    pub value: Value<'a>,
    pub span: Span,
}

/// One element of a block: either `key = value` or a bare value (list element).
#[derive(Debug, Clone, PartialEq)]
pub enum Item<'a> {
    Entry(Entry<'a>),
    Value(Value<'a>),
}

impl<'a> Item<'a> {
    pub fn span(&self) -> Span {
        match self {
            Item::Entry(e) => e.span,
            Item::Value(v) => v.span(),
        }
    }
}

/// A `{ ... }` block, or the implicit top level of a file. Items keep their source order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Block<'a> {
    pub items: Vec<Item<'a>>,
    pub span: Span,
}

impl<'a> Block<'a> {
    /// All `key = value` entries in order.
    pub fn entries(&self) -> impl Iterator<Item = &Entry<'a>> {
        self.items.iter().filter_map(|i| match i {
            Item::Entry(e) => Some(e),
            Item::Value(_) => None,
        })
    }

    /// All bare values (list elements) in order.
    pub fn values(&self) -> impl Iterator<Item = &Value<'a>> {
        self.items.iter().filter_map(|i| match i {
            Item::Value(v) => Some(v),
            Item::Entry(_) => None,
        })
    }

    /// First entry with this key.
    pub fn get(&self, key: &str) -> Option<&Entry<'a>> {
        self.entries().find(|e| e.key.text == key)
    }

    /// Every entry with this key, in source order (keys may repeat).
    pub fn get_all<'s>(&'s self, key: &'s str) -> impl Iterator<Item = &'s Entry<'a>> + 's {
        self.entries().filter(move |e| e.key.text == key)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
