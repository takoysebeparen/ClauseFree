//! Problems and coverage gaps found while loading data.
//!
//! Nothing is silently dropped: every key the loaders do not understand, every value that does not parse and every
//! broken reference becomes an [`Issue`]. The report doubles as a coverage measurement.

use std::collections::BTreeMap;
use std::fmt;

use clausewitz::DiagKind;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum IssueKind {
    /// Syntax problem in the file.
    Parse(DiagKind),
    /// A key the loader does not know. Either new game content or a coverage gap.
    UnknownKey,
    /// Understood but intentionally not interpreted yet (it needs a later layer, e.g. the script VM).
    Deferred,
    /// The same key appears more than once where only one is used.
    Duplicate,
    /// The value has the wrong shape; the payload says what was expected.
    InvalidValue(&'static str),
    /// A required key is absent.
    Missing(&'static str),
    /// A reference to something that does not exist (unknown terrain, tag, ...).
    BrokenReference(&'static str),
    /// A second definition of an object that already exists; the payload says which one was kept.
    Redefinition(&'static str),
    /// Valid syntax of the file's language that the engine does not support (yet); skipped, never guessed.
    Unsupported(&'static str),
}

impl IssueKind {
    /// Short label used for grouping.
    pub fn label(&self) -> String {
        match self {
            IssueKind::Parse(kind) => format!("parse error: {kind}"),
            IssueKind::UnknownKey => "unknown key".to_string(),
            IssueKind::Deferred => "deferred".to_string(),
            IssueKind::Duplicate => "duplicate key".to_string(),
            IssueKind::InvalidValue(expected) => format!("invalid value (expected {expected})"),
            IssueKind::Missing(what) => format!("missing {what}"),
            IssueKind::BrokenReference(what) => format!("unknown {what}"),
            IssueKind::Redefinition(kept) => format!("redefinition ({kept} kept)"),
            IssueKind::Unsupported(what) => format!("unsupported {what}"),
        }
    }

    /// Issues that mean "something is wrong" as opposed to "not implemented yet".
    pub fn is_error(&self) -> bool {
        !matches!(self, IssueKind::Deferred | IssueKind::UnknownKey)
    }
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// VFS path of the file.
    pub file: String,
    /// 1-based line, 0 if not applicable.
    pub line: usize,
    /// Schema path of the key, e.g. `state.history.set_variable`. Instance names are not part of it, so issues of
    /// the same kind group together.
    pub path: String,
    pub kind: IssueKind,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {} `{}`", self.file, self.line, self.kind.label(), self.path)
    }
}

/// All findings of a load.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub issues: Vec<Issue>,
}

impl Report {
    pub fn push(&mut self, file: &str, line: usize, path: impl Into<String>, kind: IssueKind) {
        self.issues.push(Issue { file: file.to_string(), line, path: path.into(), kind });
    }

    pub fn errors(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.kind.is_error())
    }

    pub fn count(&self, kind: &IssueKind) -> usize {
        self.issues.iter().filter(|i| &i.kind == kind).count()
    }

    /// Issues grouped by (label, schema path) with their counts, largest groups first.
    pub fn summary(&self) -> Vec<(String, String, usize)> {
        let mut groups: BTreeMap<(String, String), usize> = BTreeMap::new();
        for issue in &self.issues {
            *groups.entry((issue.kind.label(), issue.path.clone())).or_default() += 1;
        }
        let mut list: Vec<(String, String, usize)> = groups.into_iter().map(|((l, p), n)| (l, p, n)).collect();
        list.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)).then_with(|| a.1.cmp(&b.1)));
        list
    }
}
