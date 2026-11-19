//! Countries: tags (`common/country_tags/*.txt`), their country files (`common/countries/<file>`) and the colour
//! table `common/countries/colors.txt`.
//!
//! The order in which tags are defined matters in the original (it decides, among other things, the order of country
//! history evaluation). Source: community wiki, "Modding", section "Loading files". It is kept as `Countries::order`.

use std::collections::BTreeMap;

use vfs::Vfs;

use crate::report::{IssueKind, Report};
use crate::source::{Ctx, Fields, Source, evaluation_order};
use crate::types::{Color, Tag};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CountryDef {
    pub tag: Option<Tag>,
    /// Path of the country file as given in the tag list, relative to `common/`.
    pub file: String,
    /// Tags after `dynamic_tags = yes` are reserved for countries created during play (civil wars).
    pub dynamic: bool,
    pub graphical_culture: Option<String>,
    pub graphical_culture_2d: Option<String>,
    /// `use_legacy_ai_pp_spend = yes`: an AI behaviour switch.
    pub use_legacy_ai_pp_spend: Option<bool>,
    /// `color` from the country file.
    pub file_color: Option<Color>,
    /// `color` from `colors.txt`. Which colour the original uses when both exist is not settled; both are kept.
    pub map_color: Option<Color>,
    /// `color_ui` from `colors.txt`.
    pub ui_color: Option<Color>,
}

#[derive(Debug, Clone, Default)]
pub struct Countries {
    /// Tags in definition order.
    pub order: Vec<Tag>,
    pub by_tag: BTreeMap<Tag, CountryDef>,
}

impl Countries {
    pub fn get(&self, tag: Tag) -> Option<&CountryDef> {
        self.by_tag.get(&tag)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

const TAGS_DIR: &str = "common/country_tags";
const COLORS_FILE: &str = "common/countries/colors.txt";

pub fn load(vfs: &Vfs, report: &mut Report) -> Countries {
    let mut countries = Countries::default();
    for file in evaluation_order(vfs.list(TAGS_DIR).into_iter().filter(|e| e.path.ends_with(".txt")).collect()) {
        let Some(src) = Source::read(vfs, &file.path, report) else { continue };
        parse_tags(&src, report, &mut countries);
    }
    for tag in countries.order.clone() {
        let Some(def) = countries.by_tag.get_mut(&tag) else { continue };
        let path = format!("common/{}", def.file);
        if !vfs.exists(&path) {
            // Dynamic tags often have no file; report but keep the tag.
            report.push(&path, 0, "country.file", IssueKind::Missing("country file"));
            continue;
        }
        if let Some(src) = Source::read(vfs, &path, report) {
            parse_country_file(&src, report, def);
        }
    }
    if let Some(src) = Source::read(vfs, COLORS_FILE, report) {
        parse_colors(&src, report, &mut countries);
    }
    countries
}

pub fn parse_tags(src: &Source, report: &mut Report, countries: &mut Countries) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut dynamic = false;
    let mut fields = Fields::new(&parsed.root, "country_tags");
    // Keys are data here (tags), so walk entries in order instead of asking for named keys.
    for entry in fields.take_matching(|_| true) {
        if entry.key.text == "dynamic_tags" {
            dynamic = ctx.bool(entry, "country_tags.dynamic_tags").unwrap_or(false);
            continue;
        }
        let Some(tag) = Tag::parse(entry.key.text) else {
            ctx.issue(entry.key.span, "country_tags.<tag>", IssueKind::InvalidValue("a three-character tag"));
            continue;
        };
        let Some(file) = ctx.text(entry, "country_tags.<tag>") else { continue };
        if countries.by_tag.contains_key(&tag) {
            // Unmeasured which definition the original keeps; keep the first.
            ctx.issue(entry.span, "country_tags.<tag>", IssueKind::Redefinition("first"));
            continue;
        }
        countries.order.push(tag);
        countries.by_tag.insert(tag, CountryDef { tag: Some(tag), file, dynamic, ..Default::default() });
    }
    fields.finish(&mut ctx);
}

pub fn parse_country_file(src: &Source, report: &mut Report, def: &mut CountryDef) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut f = Fields::new(&parsed.root, "country");
    if let Some(e) = f.take("graphical_culture") {
        def.graphical_culture = ctx.text(e, "country.graphical_culture");
    }
    if let Some(e) = f.take("graphical_culture_2d") {
        def.graphical_culture_2d = ctx.text(e, "country.graphical_culture_2d");
    }
    if let Some(e) = f.take("use_legacy_ai_pp_spend") {
        def.use_legacy_ai_pp_spend = ctx.bool(e, "country.use_legacy_ai_pp_spend");
    }
    if let Some(e) = f.take("color") {
        def.file_color = Color::from_value(&e.value);
        if def.file_color.is_none() {
            ctx.issue(e.span, "country.color", IssueKind::InvalidValue("a colour"));
        }
    }
    f.finish(&mut ctx);
}

pub fn parse_colors(src: &Source, report: &mut Report, countries: &mut Countries) {
    let parsed = src.parse(report);
    let mut ctx = Ctx { src, report };
    let mut top = Fields::new(&parsed.root, "colors");
    for entry in top.take_matching(|_| true) {
        let Some(tag) = Tag::parse(entry.key.text) else {
            ctx.issue(entry.key.span, "colors.<tag>", IssueKind::InvalidValue("a three-character tag"));
            continue;
        };
        let Some(block) = ctx.block(entry, "colors.<tag>") else { continue };
        let Some(def) = countries.by_tag.get_mut(&tag) else {
            ctx.issue(entry.key.span, "colors.<tag>", IssueKind::BrokenReference("country tag"));
            continue;
        };
        let mut f = Fields::new(block, "colors.<tag>");
        for (key, slot) in [("color", &mut def.map_color), ("color_ui", &mut def.ui_color)] {
            if let Some(e) = f.take(key) {
                *slot = Color::from_value(&e.value);
                if slot.is_none() {
                    ctx.issue(e.span, format!("colors.<tag>.{key}"), IssueKind::InvalidValue("a colour"));
                }
            }
        }
        f.finish(&mut ctx);
    }
    top.finish(&mut ctx);
}
