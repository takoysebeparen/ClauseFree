//! Diagnostics produced while parsing. The parser never panics on bad input; it reports and recovers.

use std::fmt;

use crate::ast::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagKind {
    UnterminatedString,
    UnterminatedMath,
    /// A `{` was never closed. The span points at the opening brace.
    UnclosedBrace,
    /// A `}` with no matching `{`.
    UnexpectedCloseBrace,
    /// An operator with nothing on its left.
    UnexpectedOperator,
    /// An operator with nothing on its right.
    MissingValue,
    /// Nesting deeper than the supported limit.
    TooDeep,
}

impl fmt::Display for DiagKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DiagKind::UnterminatedString => "unterminated string",
            DiagKind::UnterminatedMath => "unterminated inline math `@[`",
            DiagKind::UnclosedBrace => "`{` is never closed",
            DiagKind::UnexpectedCloseBrace => "unexpected `}`",
            DiagKind::UnexpectedOperator => "operator without a key",
            DiagKind::MissingValue => "operator without a value",
            DiagKind::TooDeep => "nesting is too deep",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagKind,
    pub span: Span,
}

impl Diagnostic {
    pub fn new(kind: DiagKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// Maps byte offsets to 1-based line and column numbers.
#[derive(Debug, Clone)]
pub struct LineIndex {
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(u32::try_from(i + 1).unwrap_or(u32::MAX));
            }
        }
        Self { line_starts }
    }

    /// `(line, column)`, both starting at 1. Column counts bytes.
    pub fn locate(&self, offset: u32) -> (usize, usize) {
        let line = self.line_starts.partition_point(|&s| s <= offset);
        let start = self.line_starts.get(line.saturating_sub(1)).copied().unwrap_or(0);
        (line.max(1), (offset - start) as usize + 1)
    }
}
