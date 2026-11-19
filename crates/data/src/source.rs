//! Reading files from the VFS and checking parsed blocks against what a loader understands.

use clausewitz::{Block, Entry, LineIndex, Scalar, Span, Value};
use common::Fixed;
use vfs::{FileEntry, Vfs};

use crate::report::{IssueKind, Report};

/// A loaded text file with its line index, for reporting.
pub struct Source {
    pub path: String,
    pub text: String,
    lines: LineIndex,
}

impl Source {
    /// Reads a file through the VFS; a read failure is reported and yields `None`.
    pub fn read(vfs: &Vfs, path: &str, report: &mut Report) -> Option<Source> {
        match vfs.read_text(path) {
            Ok(text) => {
                let lines = LineIndex::new(&text);
                Some(Source { path: path.to_string(), text, lines })
            }
            Err(_) => {
                report.push(path, 0, "", IssueKind::Missing("readable file"));
                None
            }
        }
    }

    pub fn from_text(path: &str, text: String) -> Source {
        let lines = LineIndex::new(&text);
        Source { path: path.to_string(), text, lines }
    }

    pub fn line(&self, span: Span) -> usize {
        self.lines.locate(span.start).0
    }

    /// Parses the file, reporting syntax problems.
    pub fn parse(&self, report: &mut Report) -> clausewitz::Parsed<'_> {
        let parsed = clausewitz::parse(&self.text);
        for d in &parsed.diagnostics {
            report.push(&self.path, self.line(d.span), "", IssueKind::Parse(d.kind));
        }
        parsed
    }
}

/// Files of a directory in the original's *evaluation order*: by file name in ASCII (byte) order, so upper case
/// sorts before lower case. Source: community wiki, "Modding", section "Loading files".
pub fn evaluation_order(mut files: Vec<&FileEntry>) -> Vec<&FileEntry> {
    let name = |e: &FileEntry| e.path.rsplit('/').next().unwrap_or(&e.path).to_string();
    files.sort_by(|a, b| name(a).as_bytes().cmp(name(b).as_bytes()).then_with(|| a.path.cmp(&b.path)));
    files
}

/// Context for checking one file: where issues go and how to locate them.
pub struct Ctx<'s, 'r> {
    pub src: &'s Source,
    pub report: &'r mut Report,
}

impl Ctx<'_, '_> {
    pub fn issue(&mut self, span: Span, path: impl Into<String>, kind: IssueKind) {
        let line = self.src.line(span);
        self.report.push(&self.src.path, line, path, kind);
    }

    /// The value as a scalar, reporting otherwise.
    pub fn scalar<'a>(&mut self, entry: &Entry<'a>, path: &str) -> Option<Scalar<'a>> {
        match &entry.value {
            Value::Scalar(s) => Some(*s),
            other => {
                self.issue(other.span(), path, IssueKind::InvalidValue("a single value"));
                None
            }
        }
    }

    pub fn text(&mut self, entry: &Entry<'_>, path: &str) -> Option<String> {
        self.scalar(entry, path).map(|s| s.unescaped().into_owned())
    }

    pub fn fixed(&mut self, entry: &Entry<'_>, path: &str) -> Option<Fixed> {
        let s = self.scalar(entry, path)?;
        match Fixed::parse(s.text) {
            Ok(v) => Some(v),
            Err(_) => {
                self.issue(s.span, path, IssueKind::InvalidValue("a number"));
                None
            }
        }
    }

    pub fn int(&mut self, entry: &Entry<'_>, path: &str) -> Option<i64> {
        let s = self.scalar(entry, path)?;
        match s.as_i64() {
            Some(v) => Some(v),
            None => {
                self.issue(s.span, path, IssueKind::InvalidValue("an integer"));
                None
            }
        }
    }

    pub fn u32(&mut self, entry: &Entry<'_>, path: &str) -> Option<u32> {
        let s = self.scalar(entry, path)?;
        match s.text.parse::<u32>() {
            Ok(v) if !s.quoted => Some(v),
            _ => {
                self.issue(s.span, path, IssueKind::InvalidValue("a non-negative integer"));
                None
            }
        }
    }

    pub fn bool(&mut self, entry: &Entry<'_>, path: &str) -> Option<bool> {
        let s = self.scalar(entry, path)?;
        match s.as_bool() {
            Some(v) => Some(v),
            None => {
                self.issue(s.span, path, IssueKind::InvalidValue("yes or no"));
                None
            }
        }
    }

    pub fn block<'b, 'a>(&mut self, entry: &'b Entry<'a>, path: &str) -> Option<&'b Block<'a>> {
        match entry.value.as_block() {
            Some(b) => Some(b),
            None => {
                self.issue(entry.value.span(), path, IssueKind::InvalidValue("a block"));
                None
            }
        }
    }

    /// Bare scalars of a list block as unsigned integers (`provinces = { 1 2 3 }`).
    pub fn u32_list(&mut self, block: &Block<'_>, path: &str) -> Vec<u32> {
        let mut out = Vec::new();
        for value in block.values() {
            match value.as_scalar().filter(|s| !s.quoted).and_then(|s| s.text.parse::<u32>().ok()) {
                Some(v) => out.push(v),
                None => self.issue(value.span(), path, IssueKind::InvalidValue("a list of integers")),
            }
        }
        for entry in block.entries() {
            self.issue(entry.span, path, IssueKind::InvalidValue("a list of integers"));
        }
        out
    }
}

/// Tracks which items of a block a loader has consumed, so that everything else can be reported.
pub struct Fields<'b, 'a> {
    block: &'b Block<'a>,
    used: Vec<bool>,
    deferred: Vec<bool>,
    taken_keys: Vec<&'a str>,
    schema: String,
}

impl<'b, 'a> Fields<'b, 'a> {
    /// `schema` is the schema path of the block itself, e.g. `state` or `state.history`.
    pub fn new(block: &'b Block<'a>, schema: impl Into<String>) -> Self {
        let n = block.items.len();
        Fields { block, used: vec![false; n], deferred: vec![false; n], taken_keys: Vec::new(), schema: schema.into() }
    }

    pub fn path(&self, key: &str) -> String {
        if self.schema.is_empty() { key.to_string() } else { format!("{}.{key}", self.schema) }
    }

    fn entry_at(&self, i: usize) -> Option<&'b Entry<'a>> {
        match &self.block.items[i] {
            clausewitz::Item::Entry(e) => Some(e),
            clausewitz::Item::Value(_) => None,
        }
    }

    /// First unused entry with this key. Further entries with the same key are reported as duplicates unless taken.
    pub fn take(&mut self, key: &str) -> Option<&'b Entry<'a>> {
        for i in 0..self.used.len() {
            if self.used[i] {
                continue;
            }
            if let Some(e) = self.entry_at(i).filter(|e| e.key.text == key) {
                self.used[i] = true;
                self.taken_keys.push(e.key.text);
                return Some(e);
            }
        }
        None
    }

    /// Every unused entry with this key, in order.
    pub fn take_all(&mut self, key: &str) -> Vec<&'b Entry<'a>> {
        let mut out = Vec::new();
        while let Some(e) = self.take(key) {
            out.push(e);
        }
        out
    }

    /// Every unused entry whose key matches the predicate, in order (for keys that are data, such as dates or ids).
    pub fn take_matching(&mut self, mut pred: impl FnMut(&str) -> bool) -> Vec<&'b Entry<'a>> {
        let mut out = Vec::new();
        for i in 0..self.used.len() {
            if self.used[i] {
                continue;
            }
            if let Some(e) = self.entry_at(i).filter(|e| pred(e.key.text)) {
                self.used[i] = true;
                out.push(e);
            }
        }
        out
    }

    /// Marks every unused entry whose key matches as understood-but-deferred (reported as such, not as unknown).
    pub fn defer_matching(&mut self, mut pred: impl FnMut(&str) -> bool) {
        for i in 0..self.used.len() {
            if !self.used[i] && self.entry_at(i).is_some_and(|e| pred(e.key.text)) {
                self.used[i] = true;
                self.deferred[i] = true;
            }
        }
    }

    /// Reports everything not consumed: unknown keys, duplicates, stray list values and deferred keys.
    pub fn finish(self, ctx: &mut Ctx<'_, '_>) {
        for (i, item) in self.block.items.iter().enumerate() {
            match item {
                clausewitz::Item::Entry(e) => {
                    let path = self.path(e.key.text);
                    if self.deferred[i] {
                        ctx.issue(e.span, path, IssueKind::Deferred);
                    } else if !self.used[i] {
                        let kind = if self.taken_keys.contains(&e.key.text) {
                            IssueKind::Duplicate
                        } else {
                            IssueKind::UnknownKey
                        };
                        ctx.issue(e.span, path, kind);
                    }
                }
                clausewitz::Item::Value(v) => {
                    if !self.used[i] {
                        ctx.issue(v.span(), self.path("<value>"), IssueKind::InvalidValue("key = value"));
                    }
                }
            }
        }
    }
}
