//! Парсер Order of Battle 1936: `history/units/{TAG}_1936.txt`.
//!
//! Файлы читаются на лету из установленной игры. В реальных файлах у большинства дивизий нет
//! поля `name`: имя собирается из `division_names_group` шаблона и `name_order` по таблице
//! `common/units/names_divisions/{TAG}_names_divisions.txt` (например, `"25. Infanterie-Division"`).
//! Порядок дивизий — порядок в файле, страны — порядок `OOB_COUNTRIES`: спавн детерминирован.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::prelude::*;

use crate::map::tokenize;

/// Страны, чей OOB загружается, в порядке спавна.
pub const OOB_COUNTRIES: [&str; 2] = ["GER", "FRA"];

#[derive(Debug, Clone)]
pub struct OobDivision {
    pub name: String,
    pub owner: String,
    /// ID провинции, где дивизия появляется.
    pub location: u32,
    /// Имя `division_template` из файла OOB (пустое, если не указано).
    pub template: String,
}

/// Все распарсенные дивизии (до фильтрации по морю / центрам — она в `division.rs`).
#[derive(Resource, Default)]
pub struct OobDivisions(pub Vec<OobDivision>);

#[derive(Default)]
struct NameGroup {
    fallback: Option<String>,
    ordered: BTreeMap<u32, String>,
}

#[derive(Default)]
struct RawDivision {
    name: Option<String>,
    location: Option<u32>,
    template: Option<String>,
    order: Option<u32>,
}

// ---------------------------------------------------------------------------
// Загрузка
// ---------------------------------------------------------------------------

/// Читает OOB всех стран из `OOB_COUNTRIES`. Ошибки отдельных файлов — предупреждения.
pub fn load_oob(root: &Path) -> OobDivisions {
    let mut all = Vec::new();
    for tag in OOB_COUNTRIES {
        match load_country_oob(root, tag) {
            Ok(divisions) => {
                println!("[OOB] {tag}: распарсено дивизий: {}", divisions.len());
                all.extend(divisions);
            }
            Err(e) => eprintln!("[ПРЕДУПРЕЖДЕНИЕ] OOB {tag}: {e}"),
        }
    }
    OobDivisions(all)
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("не удалось прочитать {:?}: {e}", path))?;
    Ok(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_string())
}

fn load_country_oob(root: &Path, tag: &str) -> Result<Vec<OobDivision>, String> {
    let units = read_text(&root.join("history").join("units").join(format!("{tag}_1936.txt")))?;

    // Таблица имён необязательна: без неё имена будут «N. Шаблон».
    let names_path = root.join("common").join("units").join("names_divisions").join(format!("{tag}_names_divisions.txt"));
    let groups = match read_text(&names_path) {
        Ok(text) => parse_name_groups(&text),
        Err(e) => {
            eprintln!("[ПРЕДУПРЕЖДЕНИЕ] {e}");
            BTreeMap::new()
        }
    };

    let (templates, raws) = parse_oob(&units);
    let mut out = Vec::with_capacity(raws.len());
    for raw in raws {
        let Some(location) = raw.location else { continue }; // без location дивизию не поставить
        out.push(OobDivision {
            name: resolve_name(&raw, &templates, &groups),
            owner: tag.to_string(),
            location,
            template: raw.template.clone().unwrap_or_default(),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Парсинг
// ---------------------------------------------------------------------------

/// Ключ блока, который открывает `{` с индексом `i` (токен перед `=`), либо пустая строка.
fn block_key(tokens: &[String], i: usize) -> String {
    if i >= 2 && tokens[i - 1] == "=" { tokens[i - 2].clone() } else { String::new() }
}

/// `division_template = { name = ".." division_names_group = G ..}` и `units = { division = { .. } }`.
/// Возвращает (имя шаблона -> группа имён, сырые дивизии в порядке файла).
fn parse_oob(text: &str) -> (BTreeMap<String, Option<String>>, Vec<RawDivision>) {
    let tokens = tokenize(text);
    let mut stack: Vec<String> = Vec::new();
    let mut templates: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut divisions = Vec::new();
    let mut tpl_name: Option<String> = None;
    let mut tpl_group: Option<String> = None;
    let mut current = RawDivision::default();

    for i in 0..tokens.len() {
        let tok = tokens[i].as_str();
        match tok {
            "{" => {
                stack.push(block_key(&tokens, i));
                match stack.as_slice() {
                    [t] if t == "division_template" => {
                        tpl_name = None;
                        tpl_group = None;
                    }
                    [u, d] if u == "units" && d == "division" => current = RawDivision::default(),
                    _ => {}
                }
            }
            "}" => {
                match stack.as_slice() {
                    [t] if t == "division_template" => {
                        if let Some(name) = tpl_name.take() {
                            templates.insert(name, tpl_group.take());
                        }
                    }
                    [u, d] if u == "units" && d == "division" => divisions.push(std::mem::take(&mut current)),
                    _ => {}
                }
                stack.pop();
            }
            "=" => {}
            key => {
                if tokens.get(i + 1).map(String::as_str) != Some("=") {
                    continue;
                }
                let Some(value) = tokens.get(i + 2).filter(|t| !matches!(t.as_str(), "{" | "}")) else { continue };
                match (stack.as_slice(), key) {
                    ([t], "name") if t == "division_template" => tpl_name = Some(value.clone()),
                    ([t], "division_names_group") if t == "division_template" => tpl_group = Some(value.clone()),
                    ([u, d], "name") if u == "units" && d == "division" => current.name = Some(value.clone()),
                    ([u, d], "location") if u == "units" && d == "division" => current.location = value.parse().ok(),
                    ([u, d], "division_template") if u == "units" && d == "division" => {
                        current.template = Some(value.clone())
                    }
                    ([u, d, n], "name_order") if u == "units" && d == "division" && n == "division_name" => {
                        current.order = value.parse().ok()
                    }
                    _ => {}
                }
            }
        }
    }
    (templates, divisions)
}

/// `GROUP = { fallback_name = ".." ordered = { N = { "%d. Name" } ... } }`.
fn parse_name_groups(text: &str) -> BTreeMap<String, NameGroup> {
    let tokens = tokenize(text);
    let mut stack: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, NameGroup> = BTreeMap::new();

    for i in 0..tokens.len() {
        let tok = tokens[i].as_str();
        match tok {
            "{" => stack.push(block_key(&tokens, i)),
            "}" => {
                stack.pop();
            }
            "=" => {}
            key => {
                let is_assign = tokens.get(i + 1).map(String::as_str) == Some("=");
                if is_assign {
                    let value = tokens.get(i + 2).filter(|t| !matches!(t.as_str(), "{" | "}"));
                    if let ([group], "fallback_name", Some(v)) = (stack.as_slice(), key, value) {
                        groups.entry(group.clone()).or_default().fallback = Some(v.clone());
                    }
                } else if let [group, ordered, n] = stack.as_slice() {
                    // первая строка внутри `N = { "%d. Name" ... }` — само имя
                    if ordered == "ordered" {
                        if let Ok(n) = n.parse::<u32>() {
                            groups.entry(group.clone()).or_default().ordered.entry(n).or_insert_with(|| key.to_string());
                        }
                    }
                }
            }
        }
    }
    groups
}

// ---------------------------------------------------------------------------
// Имена
// ---------------------------------------------------------------------------

fn roman(mut n: u32) -> String {
    const TABLE: [(u32, &str); 13] = [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"),
        (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ];
    let mut s = String::new();
    for (value, sym) in TABLE {
        while n >= value {
            s.push_str(sym);
            n -= value;
        }
    }
    s
}

/// Подставляет номер: `%d` — арабский, `%s` — римский.
fn format_name(pattern: &str, order: u32) -> String {
    pattern.replace("%d", &order.to_string()).replace("%s", &roman(order))
}

/// Явное `name` -> имя из таблицы по `name_order` -> `fallback_name` группы -> «N. Шаблон».
fn resolve_name(
    raw: &RawDivision,
    templates: &BTreeMap<String, Option<String>>,
    groups: &BTreeMap<String, NameGroup>,
) -> String {
    if let Some(name) = &raw.name {
        return name.clone();
    }
    let template = raw.template.as_deref().unwrap_or("Division");
    let group = templates.get(template).and_then(|g| g.as_ref()).and_then(|g| groups.get(g));
    if let (Some(order), Some(group)) = (raw.order, group) {
        if let Some(pattern) = group.ordered.get(&order) {
            return format_name(pattern, order);
        }
        if let Some(pattern) = &group.fallback {
            return format_name(pattern, order);
        }
    }
    match raw.order {
        Some(order) => format!("{order}. {template}"),
        None => template.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Проверка на реальных файлах игры; без установленной игры тест пропускается.
    #[test]
    fn parses_real_oob() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let oob = load_oob(root);
        for tag in OOB_COUNTRIES {
            let list: Vec<_> = oob.0.iter().filter(|d| d.owner == tag).collect();
            println!("{tag}: {}", list.len());
            for d in list.iter().take(4) {
                println!("  {:?} @ {}", d.name, d.location);
            }
            assert!(!list.is_empty(), "{tag}: нет дивизий");
        }
    }
}
