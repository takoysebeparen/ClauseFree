//! Recursive-descent parser with error recovery.
//!
//! Grammar (informal):
//!
//! ```text
//! block := item*
//! item  := scalar op value        -- entry
//!        | scalar block           -- tagged value, e.g. `rgb { 1 2 3 }`
//!        | scalar | block         -- bare list elements
//! value := scalar | block | scalar block
//! ```

use crate::ast::{Block, Entry, Item, Scalar, Span, Value};
use crate::diag::{DiagKind, Diagnostic};
use crate::lexer::{Tok, Token, tokenize};

/// Maximum block nesting. Real files stay far below this; the limit protects the stack.
const MAX_DEPTH: usize = 256;

/// Result of parsing one file: the tree (as complete as the input allows) and any problems found.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed<'a> {
    pub root: Block<'a>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Parsed<'_> {
    pub fn is_clean(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Parses script text. Never panics; malformed input yields diagnostics and a best-effort tree.
pub fn parse(src: &str) -> Parsed<'_> {
    let mut diagnostics = Vec::new();
    let tokens = tokenize(src, &mut diagnostics);
    let mut parser = Parser { src, tokens, pos: 0, diagnostics, depth: 0 };
    let root = parser.parse_block(Span::default(), true);
    Parsed { root, diagnostics: parser.diagnostics }
}

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    diagnostics: Vec<Diagnostic>,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Token {
        // The token list always ends with Eof, so clamping is enough.
        self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn bump(&mut self) -> Token {
        let t = self.peek();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn text(&self, span: Span) -> &'a str {
        self.src.get(span.start as usize..span.end as usize).unwrap_or("")
    }

    fn scalar(&self, tok: Token) -> Scalar<'a> {
        Scalar { text: self.text(tok.span), quoted: tok.kind == Tok::Quoted, span: tok.span }
    }

    fn report(&mut self, kind: DiagKind, span: Span) {
        self.diagnostics.push(Diagnostic::new(kind, span));
    }

    /// Parses items until the matching `}` (consumed) or end of input. `open` is the span of the `{`.
    fn parse_block(&mut self, open: Span, top: bool) -> Block<'a> {
        let mut items = Vec::new();
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.report(DiagKind::TooDeep, open);
            // Skip to the matching close so the rest of the file is still parsed.
            self.skip_balanced();
            self.depth -= 1;
            return Block { items, span: open };
        }
        let end = loop {
            let tok = self.peek();
            match tok.kind {
                Tok::Eof => {
                    if !top {
                        self.report(DiagKind::UnclosedBrace, open);
                    }
                    break tok.span.start;
                }
                Tok::RBrace => {
                    if top {
                        self.report(DiagKind::UnexpectedCloseBrace, tok.span);
                        self.bump();
                        continue;
                    }
                    self.bump();
                    break tok.span.end;
                }
                Tok::Op(_) => {
                    self.report(DiagKind::UnexpectedOperator, tok.span);
                    self.bump();
                }
                Tok::LBrace => {
                    self.bump();
                    let block = self.parse_block(tok.span, false);
                    items.push(Item::Value(Value::Block(block)));
                }
                Tok::Bare | Tok::Quoted => {
                    self.bump();
                    items.push(self.parse_after_scalar(tok));
                }
            }
        };
        self.depth -= 1;
        let span = if top { Span::new(0, self.src.len()) } else { Span { start: open.start, end } };
        Block { items, span }
    }

    /// An item that began with a scalar: decide between entry, tagged block and bare value.
    fn parse_after_scalar(&mut self, first: Token) -> Item<'a> {
        let key = self.scalar(first);
        let next = self.peek();
        match next.kind {
            Tok::Op(op) => {
                self.bump();
                let value = self.parse_value(next.span);
                let span = key.span.to(value.span());
                Item::Entry(Entry { key, op, value, span })
            }
            Tok::LBrace => {
                self.bump();
                let block = self.parse_block(next.span, false);
                Item::Value(Value::Tagged(key, block))
            }
            _ => Item::Value(Value::Scalar(key)),
        }
    }

    /// Value after an operator. `op_span` is used to place a "missing value" diagnostic.
    fn parse_value(&mut self, op_span: Span) -> Value<'a> {
        let tok = self.peek();
        match tok.kind {
            Tok::LBrace => {
                self.bump();
                Value::Block(self.parse_block(tok.span, false))
            }
            Tok::Bare | Tok::Quoted => {
                self.bump();
                let scalar = self.scalar(tok);
                if self.peek().kind == Tok::LBrace {
                    let open = self.bump();
                    let block = self.parse_block(open.span, false);
                    Value::Tagged(scalar, block)
                } else {
                    Value::Scalar(scalar)
                }
            }
            Tok::RBrace | Tok::Eof | Tok::Op(_) => {
                self.report(DiagKind::MissingValue, op_span);
                Value::Missing(op_span)
            }
        }
    }

    /// Consumes tokens up to and including the `}` that closes the block we are inside (we are past its `{`).
    fn skip_balanced(&mut self) {
        let mut level = 1usize;
        loop {
            match self.bump().kind {
                Tok::LBrace => level += 1,
                Tok::RBrace => {
                    level -= 1;
                    if level == 0 {
                        break;
                    }
                }
                Tok::Eof => break,
                _ => {}
            }
        }
    }
}
