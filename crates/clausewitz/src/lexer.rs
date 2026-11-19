//! Tokenizer for Paradox script.
//!
//! Works on bytes: every delimiter is ASCII, and ASCII bytes never occur inside a multi-byte UTF-8 sequence, so all
//! token boundaries are valid `str` boundaries.

use crate::ast::{Operator, Span};
use crate::diag::{DiagKind, Diagnostic};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tok {
    LBrace,
    RBrace,
    Op(Operator),
    /// Unquoted word, number or `@[...]` inline math expression.
    Bare,
    /// Quoted string; the token span covers the text *between* the quotes.
    Quoted,
    Eof,
}

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: Tok,
    pub span: Span,
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c)
}

/// Bytes that end a bare word. `?` and `!` end it only when they begin `?=` / `!=` (checked separately).
fn is_delimiter(b: u8) -> bool {
    is_space(b) || matches!(b, b'{' | b'}' | b'=' | b'<' | b'>' | b'"' | b'#')
}

/// Splits `src` into tokens. Always ends with `Tok::Eof`. Problems (unterminated strings or math) are reported in
/// `diags`; the lexer never stops early.
pub fn tokenize(src: &str, diags: &mut Vec<Diagnostic>) -> Vec<Token> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::with_capacity(bytes.len() / 6 + 4);
    let mut i = 0;
    // UTF-8 byte order mark.
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        i = 3;
    }
    let push = |tokens: &mut Vec<Token>, kind: Tok, start: usize, end: usize| {
        tokens.push(Token { kind, span: Span::new(start, end) });
    };

    while i < bytes.len() {
        let b = bytes[i];
        if is_space(b) {
            i += 1;
            continue;
        }
        match b {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' => {
                push(&mut tokens, Tok::LBrace, i, i + 1);
                i += 1;
            }
            b'}' => {
                push(&mut tokens, Tok::RBrace, i, i + 1);
                i += 1;
            }
            b'"' => {
                let start = i + 1;
                let mut j = start;
                let mut closed = false;
                // A newline ends an unterminated string (community wiki, "Modding": "A space will not interrupt the
                // string, but a direct newline will"), so one stray quote cannot swallow the rest of the file.
                while j < bytes.len() {
                    match bytes[j] {
                        b'\n' => break,
                        b'\\' if bytes.get(j + 1).is_some_and(|&c| c != b'\n') => j += 2,
                        b'"' => {
                            closed = true;
                            break;
                        }
                        _ => j += 1,
                    }
                }
                let mut end = j.min(bytes.len());
                if !closed && end > start && bytes[end - 1] == b'\r' {
                    end -= 1;
                }
                if !closed {
                    diags.push(Diagnostic::new(DiagKind::UnterminatedString, Span::new(i, end)));
                }
                push(&mut tokens, Tok::Quoted, start, end);
                i = if closed { end + 1 } else { end };
            }
            b'=' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    push(&mut tokens, Tok::Op(Operator::EqEq), i, i + 2);
                    i += 2;
                } else {
                    push(&mut tokens, Tok::Op(Operator::Eq), i, i + 1);
                    i += 1;
                }
            }
            b'<' | b'>' => {
                let with_eq = bytes.get(i + 1) == Some(&b'=');
                let op = match (b, with_eq) {
                    (b'<', false) => Operator::Lt,
                    (b'<', true) => Operator::Le,
                    (_, false) => Operator::Gt,
                    (_, true) => Operator::Ge,
                };
                let len = if with_eq { 2 } else { 1 };
                push(&mut tokens, Tok::Op(op), i, i + len);
                i += len;
            }
            b'!' if bytes.get(i + 1) == Some(&b'=') => {
                push(&mut tokens, Tok::Op(Operator::NotEq), i, i + 2);
                i += 2;
            }
            b'?' if bytes.get(i + 1) == Some(&b'=') => {
                push(&mut tokens, Tok::Op(Operator::OptionalEq), i, i + 2);
                i += 2;
            }
            _ => {
                let start = i;
                // Inline math `@[ ... ]` may contain spaces and operators.
                if b == b'@' && bytes.get(i + 1) == Some(&b'[') {
                    let mut j = i + 2;
                    while j < bytes.len() && bytes[j] != b']' {
                        j += 1;
                    }
                    if j >= bytes.len() {
                        diags.push(Diagnostic::new(DiagKind::UnterminatedMath, Span::new(start, bytes.len())));
                        i = bytes.len();
                    } else {
                        i = j + 1;
                    }
                    push(&mut tokens, Tok::Bare, start, i);
                    continue;
                }
                let mut j = i;
                while j < bytes.len() {
                    let c = bytes[j];
                    if is_delimiter(c) || ((c == b'!' || c == b'?') && bytes.get(j + 1) == Some(&b'=')) {
                        break;
                    }
                    j += 1;
                }
                // `j == i` cannot happen: the first byte is not a delimiter (handled above), but guarantee progress.
                if j == i {
                    j = i + 1;
                }
                push(&mut tokens, Tok::Bare, start, j);
                i = j;
            }
        }
    }
    push(&mut tokens, Tok::Eof, bytes.len(), bytes.len());
    tokens
}
