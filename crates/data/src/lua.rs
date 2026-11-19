//! Reader for the small subset of Lua used by `common/defines/*.lua`.
//!
//! This is not a Lua interpreter. It understands:
//!
//! - assignments `Name = <value>` and `A.B.C = <value>` (mods override single defines this way);
//! - values: numbers (with an optional leading `-`), strings in `"` or `'`, `true`, `false`, `nil`, and tables
//!   `{ ... }` with `key = value`, `["key"] = value`, `[1] = value` and plain list elements, separated by `,` or `;`;
//! - comments `-- ...` and long comments `--[[ ... ]]` / `--[==[ ... ]==]`;
//! - the merge idiom found in the game: `for k,v in pairs( A ) do B[k] = v end` (copies the fields of `A` into `B`).
//!
//! Anything else (functions, conditions, arithmetic) is reported as unsupported and skipped; it is never guessed.
//! Numbers become [`Fixed`]; a number that does not fit is reported.

use std::collections::BTreeMap;

use clausewitz::Span;
use common::Fixed;

/// A Lua value as found in defines.
#[derive(Debug, Clone, PartialEq)]
pub enum LuaValue {
    Number(Fixed),
    String(String),
    Bool(bool),
    Nil,
    Table(LuaTable),
}

impl LuaValue {
    pub fn as_number(&self) -> Option<Fixed> {
        match self {
            LuaValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            LuaValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            LuaValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&LuaTable> {
        match self {
            LuaValue::Table(t) => Some(t),
            _ => None,
        }
    }
}

/// A table: named fields in definition order plus list elements.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LuaTable {
    pub fields: Vec<(String, LuaValue)>,
    pub list: Vec<LuaValue>,
}

impl LuaTable {
    pub fn get(&self, key: &str) -> Option<&LuaValue> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut LuaValue> {
        self.fields.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Sets a field, replacing an existing one in place (keeps the original position).
    pub fn set(&mut self, key: &str, value: LuaValue) {
        match self.get_mut(key) {
            Some(slot) => *slot = value,
            None => self.fields.push((key.to_string(), value)),
        }
    }

    /// Number of non-table values below this table (fields and list elements, recursively).
    pub fn leaf_count(&self) -> usize {
        let count = |v: &LuaValue| match v {
            LuaValue::Table(t) => t.leaf_count(),
            _ => 1,
        };
        self.fields.iter().map(|(_, v)| count(v)).sum::<usize>() + self.list.iter().map(count).sum::<usize>()
    }
}

/// A problem found while reading; `span` points into the source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaIssue {
    pub span: Span,
    pub message: String,
    /// The construct is valid Lua but outside the supported subset (as opposed to broken text).
    pub unsupported: bool,
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    Number(String),
    Str(String),
    Sym(char),
    Eof,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    span: Span,
}

fn lex(src: &str, issues: &mut Vec<LuaIssue>) -> Vec<Token> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
            i += 2;
            // Long comment `--[[ ... ]]` or `--[==[ ... ]==]`.
            if bytes.get(i) == Some(&b'[') {
                let mut j = i + 1;
                while bytes.get(j) == Some(&b'=') {
                    j += 1;
                }
                if bytes.get(j) == Some(&b'[') {
                    let level = j - i - 1;
                    let close = format!("]{}]", "=".repeat(level));
                    match src[j + 1..].find(&close) {
                        Some(pos) => i = j + 1 + pos + close.len(),
                        None => {
                            issues.push(LuaIssue {
                                span: Span::new(start, bytes.len()),
                                message: "unterminated long comment".into(),
                                unsupported: false,
                            });
                            i = bytes.len();
                        }
                    }
                    continue;
                }
            }
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'"' || b == b'\'' {
            let mut text = String::new();
            let mut j = i + 1;
            let mut closed = false;
            while j < bytes.len() {
                let c = bytes[j];
                if c == b {
                    closed = true;
                    break;
                }
                if c == b'\n' {
                    break;
                }
                if c == b'\\' && j + 1 < bytes.len() {
                    let e = bytes[j + 1];
                    text.push(match e {
                        b'n' => '\n',
                        b't' => '\t',
                        other => other as char,
                    });
                    j += 2;
                    continue;
                }
                // Copy one UTF-8 character.
                let ch_len = src[j..].chars().next().map_or(1, char::len_utf8);
                text.push_str(&src[j..j + ch_len]);
                j += ch_len;
            }
            if !closed {
                issues.push(LuaIssue {
                    span: Span::new(start, j),
                    message: "unterminated string".into(),
                    unsupported: false,
                });
            }
            i = if closed { j + 1 } else { j };
            out.push(Token { tok: Tok::Str(text), span: Span::new(start, i) });
            continue;
        }
        if b.is_ascii_digit() || (b == b'.' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'.') {
                j += 1;
            }
            out.push(Token { tok: Tok::Number(src[i..j].to_string()), span: Span::new(i, j) });
            i = j;
            continue;
        }
        if b.is_ascii_alphabetic() || b == b'_' {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            out.push(Token { tok: Tok::Name(src[i..j].to_string()), span: Span::new(i, j) });
            i = j;
            continue;
        }
        let ch = src[i..].chars().next().unwrap_or('?');
        out.push(Token { tok: Tok::Sym(ch), span: Span::new(i, i + ch.len_utf8()) });
        i += ch.len_utf8();
    }
    out.push(Token { tok: Tok::Eof, span: Span::new(bytes.len(), bytes.len()) });
    out
}

// ---------------------------------------------------------------------------
// Parser / evaluator
// ---------------------------------------------------------------------------

/// Global variables after running one or more files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LuaEnv {
    pub globals: BTreeMap<String, LuaValue>,
}

/// Maximum table nesting; protects the stack against malicious input.
const MAX_DEPTH: usize = 64;

struct Parser<'e> {
    toks: Vec<Token>,
    pos: usize,
    issues: Vec<LuaIssue>,
    env: &'e mut LuaEnv,
}

type Fail = (Span, String, bool);

impl Parser<'_> {
    fn peek(&self) -> &Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn bump(&mut self) -> Token {
        let t = self.peek().clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn expect_sym(&mut self, c: char) -> Result<Span, Fail> {
        let t = self.bump();
        if t.tok == Tok::Sym(c) { Ok(t.span) } else { Err((t.span, format!("expected `{c}`"), false)) }
    }

    fn expect_name(&mut self) -> Result<(String, Span), Fail> {
        let t = self.bump();
        match t.tok {
            Tok::Name(n) => Ok((n, t.span)),
            _ => Err((t.span, "expected a name".into(), false)),
        }
    }

    fn run(&mut self) {
        while self.peek().tok != Tok::Eof {
            let start = self.pos;
            if let Err((span, message, unsupported)) = self.statement() {
                self.issues.push(LuaIssue { span, message, unsupported });
                self.recover(start);
            }
        }
    }

    /// Skips to the next plausible statement start: a name at bracket depth 0 followed by `=` or `.`.
    fn recover(&mut self, start: usize) {
        if self.pos == start {
            self.bump();
        }
        let mut depth: i64 = 0;
        loop {
            match self.peek().tok.clone() {
                Tok::Eof => return,
                Tok::Sym('{') | Tok::Sym('(') | Tok::Sym('[') => depth += 1,
                Tok::Sym('}') | Tok::Sym(')') | Tok::Sym(']') => depth = (depth - 1).max(0),
                Tok::Name(_) if depth == 0 && matches!(self.peek_at(1), Tok::Sym('=') | Tok::Sym('.')) => return,
                _ => {}
            }
            self.bump();
        }
    }

    fn statement(&mut self) -> Result<(), Fail> {
        let first = self.peek().clone();
        match &first.tok {
            Tok::Name(n) if n == "for" => self.merge_loop(),
            Tok::Name(n) if matches!(n.as_str(), "local" | "function" | "if" | "while" | "return" | "repeat") => {
                Err((first.span, format!("`{n}` is not supported in defines"), true))
            }
            Tok::Name(_) => self.assignment(),
            _ => Err((first.span, "expected a statement".into(), false)),
        }
    }

    /// `A.B["C"] = value`
    fn assignment(&mut self) -> Result<(), Fail> {
        let (root, root_span) = self.expect_name()?;
        let mut path = vec![root];
        loop {
            match self.peek().tok {
                Tok::Sym('.') => {
                    self.bump();
                    path.push(self.expect_name()?.0);
                }
                Tok::Sym('[') => {
                    self.bump();
                    let key = self.bump();
                    let k = match key.tok {
                        Tok::Str(s) | Tok::Number(s) => s,
                        _ => return Err((key.span, "expected a string or number key".into(), true)),
                    };
                    self.expect_sym(']')?;
                    path.push(k);
                }
                _ => break,
            }
        }
        let eq = self.peek().clone();
        if eq.tok != Tok::Sym('=') {
            return Err((eq.span, "expected `=`".into(), eq.tok == Tok::Sym('(')));
        }
        self.bump();
        let value = self.value(0)?;
        self.assign(&path, value).map_err(|m| (root_span, m, false))
    }

    fn assign(&mut self, path: &[String], value: LuaValue) -> Result<(), String> {
        let (last, parents) = path.split_last().ok_or("empty assignment")?;
        if parents.is_empty() {
            self.env.globals.insert(last.clone(), value);
            return Ok(());
        }
        let mut current =
            self.env.globals.get_mut(&parents[0]).ok_or_else(|| format!("`{}` is not defined", parents[0]))?;
        for key in &parents[1..] {
            current = match current {
                LuaValue::Table(t) => t.get_mut(key).ok_or_else(|| format!("`{key}` is not defined"))?,
                _ => return Err(format!("`{key}` is not a table")),
            };
        }
        match current {
            LuaValue::Table(t) => {
                t.set(last, value);
                Ok(())
            }
            _ => Err(format!("cannot set `{last}`: not a table")),
        }
    }

    /// `for k,v in pairs( A ) do B[k] = v end`
    fn merge_loop(&mut self) -> Result<(), Fail> {
        let for_span = self.bump().span;
        let unsupported =
            |span: Span| (span, "only `for k,v in pairs(A) do B[k] = v end` is supported".to_string(), true);
        let (k, _) = self.expect_name().map_err(|e| unsupported(e.0))?;
        self.expect_sym(',').map_err(|e| unsupported(e.0))?;
        let (v, _) = self.expect_name().map_err(|e| unsupported(e.0))?;
        for word in ["in", "pairs"] {
            let (n, span) = self.expect_name().map_err(|e| unsupported(e.0))?;
            if n != word {
                return Err(unsupported(span));
            }
        }
        self.expect_sym('(').map_err(|e| unsupported(e.0))?;
        let (source, _) = self.expect_name().map_err(|e| unsupported(e.0))?;
        self.expect_sym(')').map_err(|e| unsupported(e.0))?;
        let (do_word, span) = self.expect_name().map_err(|e| unsupported(e.0))?;
        if do_word != "do" {
            return Err(unsupported(span));
        }
        let (target, _) = self.expect_name().map_err(|e| unsupported(e.0))?;
        self.expect_sym('[').map_err(|e| unsupported(e.0))?;
        let (k2, _) = self.expect_name().map_err(|e| unsupported(e.0))?;
        self.expect_sym(']').map_err(|e| unsupported(e.0))?;
        self.expect_sym('=').map_err(|e| unsupported(e.0))?;
        let (v2, _) = self.expect_name().map_err(|e| unsupported(e.0))?;
        let (end_word, span) = self.expect_name().map_err(|e| unsupported(e.0))?;
        if end_word != "end" || k2 != k || v2 != v {
            return Err(unsupported(span));
        }
        let fields = match self.env.globals.get(&source) {
            Some(LuaValue::Table(t)) => t.fields.clone(),
            _ => return Err((for_span, format!("`{source}` is not a table"), false)),
        };
        match self.env.globals.get_mut(&target) {
            Some(LuaValue::Table(t)) => {
                for (key, value) in fields {
                    t.set(&key, value);
                }
                Ok(())
            }
            _ => Err((for_span, format!("`{target}` is not a table"), false)),
        }
    }

    /// A value: a table, a string, a boolean, `nil`, or a numeric expression with `+ - * /` and parentheses
    /// (the game writes constants such as `7 * 24`).
    fn value(&mut self, depth: usize) -> Result<LuaValue, Fail> {
        let t = self.peek().clone();
        match t.tok {
            Tok::Str(s) => {
                self.bump();
                Ok(LuaValue::String(s))
            }
            Tok::Name(ref n) if n == "true" || n == "false" || n == "nil" => {
                self.bump();
                Ok(match n.as_str() {
                    "true" => LuaValue::Bool(true),
                    "false" => LuaValue::Bool(false),
                    _ => LuaValue::Nil,
                })
            }
            Tok::Sym('{') => {
                self.bump();
                self.table(t.span, depth + 1)
            }
            _ => self.sum(depth).map(LuaValue::Number),
        }
    }

    fn sum(&mut self, depth: usize) -> Result<Fixed, Fail> {
        let mut acc = self.product(depth)?;
        while let Tok::Sym(op @ ('+' | '-')) = self.peek().tok {
            self.bump();
            let rhs = self.product(depth)?;
            acc = if op == '+' { acc + rhs } else { acc - rhs };
        }
        Ok(acc)
    }

    fn product(&mut self, depth: usize) -> Result<Fixed, Fail> {
        let mut acc = self.factor(depth)?;
        while let Tok::Sym(op @ ('*' | '/')) = self.peek().tok {
            let op_span = self.bump().span;
            let rhs = self.factor(depth)?;
            acc = if op == '*' {
                acc * rhs
            } else {
                acc.checked_div(rhs).ok_or((op_span, "division by zero".to_string(), false))?
            };
        }
        Ok(acc)
    }

    fn factor(&mut self, depth: usize) -> Result<Fixed, Fail> {
        if depth > MAX_DEPTH {
            return Err((self.peek().span, "expression nested too deeply".into(), false));
        }
        let t = self.bump();
        match t.tok {
            Tok::Number(text) => match number(&text, t.span)? {
                LuaValue::Number(n) => Ok(n),
                _ => Err((t.span, "expected a number".into(), false)),
            },
            Tok::Sym('-') => Ok(-self.factor(depth + 1)?),
            Tok::Sym('(') => {
                let v = self.sum(depth + 1)?;
                self.expect_sym(')')?;
                Ok(v)
            }
            Tok::Name(n) => Err((t.span, format!("references such as `{n}` are not supported"), true)),
            _ => Err((t.span, "expected a value".into(), false)),
        }
    }

    /// After a bad table element: skip to the next `,`/`;` or to the `}` closing this table (not consumed).
    fn skip_element(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.peek().tok {
                Tok::Eof => return,
                Tok::Sym('{') | Tok::Sym('(') | Tok::Sym('[') => depth += 1,
                Tok::Sym('}') | Tok::Sym(')') | Tok::Sym(']') => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                }
                Tok::Sym(',') | Tok::Sym(';') if depth == 0 => return,
                _ => {}
            }
            self.bump();
        }
    }

    /// Reads one table element value; on failure records the problem and skips the element.
    fn element(&mut self, depth: usize) -> Option<LuaValue> {
        match self.value(depth) {
            Ok(v) => Some(v),
            Err((span, message, unsupported)) => {
                self.issues.push(LuaIssue { span, message, unsupported });
                self.skip_element();
                None
            }
        }
    }

    fn table(&mut self, open: Span, depth: usize) -> Result<LuaValue, Fail> {
        if depth > MAX_DEPTH {
            return Err((open, "tables nested too deeply".into(), false));
        }
        let mut table = LuaTable::default();
        loop {
            let t = self.peek().clone();
            match t.tok {
                Tok::Sym('}') => {
                    self.bump();
                    return Ok(LuaValue::Table(table));
                }
                Tok::Eof => return Err((open, "`{` is never closed".into(), false)),
                Tok::Sym(',') | Tok::Sym(';') => {
                    self.bump();
                }
                Tok::Name(ref n) if *self.peek_at(1) == Tok::Sym('=') => {
                    let key = n.clone();
                    self.bump();
                    self.bump();
                    if let Some(v) = self.element(depth) {
                        table.set(&key, v);
                    }
                }
                Tok::Sym('[') => {
                    self.bump();
                    let key = self.bump();
                    let k = match key.tok {
                        Tok::Str(s) | Tok::Number(s) => s,
                        _ => return Err((key.span, "expected a string or number key".into(), true)),
                    };
                    self.expect_sym(']')?;
                    self.expect_sym('=')?;
                    if let Some(v) = self.element(depth) {
                        table.set(&k, v);
                    }
                }
                _ => {
                    let before = self.pos;
                    if let Some(v) = self.element(depth) {
                        table.list.push(v);
                    }
                    // A token that cannot start an element must not stall the loop.
                    if self.pos == before {
                        self.bump();
                    }
                }
            }
        }
    }
}

fn number(text: &str, span: Span) -> Result<LuaValue, Fail> {
    Fixed::parse(text).map(LuaValue::Number).map_err(|e| (span, format!("number `{text}`: {e}"), false))
}

/// Runs one file's statements against `env`. Returns the problems found; never panics.
pub fn run(src: &str, env: &mut LuaEnv) -> Vec<LuaIssue> {
    let mut issues = Vec::new();
    let toks = lex(src, &mut issues);
    let mut parser = Parser { toks, pos: 0, issues, env };
    parser.run();
    parser.issues
}
