//! Engine constants: `common/defines/*.lua`.
//!
//! The base game defines three tables, `NDefines` (game rules, combat, economy, AI, ...), `NDefines_Graphics` and
//! `NDefines_CareerProfile` (merged into `NDefines` by a loop at the end of its file). Mods override single values
//! with lines such as `NDefines.NGame.START_DATE = "1937.1.1.12"` in their own `.lua` files. All files are run in
//! evaluation order (ASCII by file name) against one environment, so later assignments win, which is how the
//! original applies mod overrides (a mod file runs after `00_defines.lua` because its name sorts later; a mod file
//! named exactly like a base file replaces it through the VFS).
//!
//! Source: the files themselves; the evaluation order rule is from the community wiki ("Modding", "Loading files").

use common::Fixed;
use vfs::Vfs;

use crate::lua::{self, LuaEnv, LuaTable, LuaValue};
use crate::report::{IssueKind, Report};
use crate::source::{Source, evaluation_order};

#[derive(Debug, Clone, Default)]
pub struct Defines {
    pub env: LuaEnv,
}

impl Defines {
    /// Value at a dotted path from a global, e.g. `NDefines.NGame.START_DATE`.
    pub fn get(&self, path: &str) -> Option<&LuaValue> {
        let mut parts = path.split('.');
        let mut current = self.env.globals.get(parts.next()?)?;
        for key in parts {
            current = current.as_table()?.get(key)?;
        }
        Some(current)
    }

    /// `NDefines.<section>.<key>`.
    pub fn define(&self, section: &str, key: &str) -> Option<&LuaValue> {
        self.get("NDefines")
            .and_then(LuaValue::as_table)
            .and_then(|t| t.get(section))
            .and_then(LuaValue::as_table)?
            .get(key)
    }

    pub fn number(&self, path: &str) -> Option<Fixed> {
        self.get(path).and_then(LuaValue::as_number)
    }

    pub fn string(&self, path: &str) -> Option<&str> {
        self.get(path).and_then(LuaValue::as_str)
    }

    pub fn bool(&self, path: &str) -> Option<bool> {
        self.get(path).and_then(LuaValue::as_bool)
    }

    /// The table of a section, e.g. `NDefines.NMilitary`.
    pub fn section(&self, path: &str) -> Option<&LuaTable> {
        self.get(path).and_then(LuaValue::as_table)
    }

    /// Number of single values (not tables) defined across all globals.
    pub fn len(&self) -> usize {
        self.env
            .globals
            .values()
            .map(|v| match v {
                LuaValue::Table(t) => t.leaf_count(),
                _ => 1,
            })
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

const DIR: &str = "common/defines";

pub fn load(vfs: &Vfs, report: &mut Report) -> Defines {
    let mut defines = Defines::default();
    for file in
        evaluation_order(vfs.list(DIR).into_iter().filter(|e| e.path.to_lowercase().ends_with(".lua")).collect())
    {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        run_file(&src, report, &mut defines);
    }
    defines
}

/// Runs one defines file into `defines`, reporting problems.
pub fn run_file(src: &Source, report: &mut Report, defines: &mut Defines) {
    for issue in lua::run(&src.text, &mut defines.env) {
        let kind =
            if issue.unsupported { IssueKind::Unsupported("Lua") } else { IssueKind::InvalidValue("Lua defines") };
        report.push(&src.path, src.line(issue.span), format!("defines: {}", issue.message), kind);
    }
}
