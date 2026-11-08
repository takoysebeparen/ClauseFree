//! Парсер Order of Battle 1936: `history/units/{TAG}_1936.txt`.
//!
//! Файлы читаются на лету из установленной игры. В реальных файлах у большинства дивизий нет
//! поля `name`: имя собирается из `division_names_group` шаблона и `name_order` по таблице
//! `common/units/names_divisions/{TAG}_names_divisions.txt` (например, `"25. Infanterie-Division"`).
//! Порядок дивизий — порядок в файле, страны — порядок `OOB_COUNTRIES`: спавн детерминирован.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::prelude::*;

use crate::map::{tokenize, StateRegistry};
use crate::units::{DivisionTemplates, BASE_TEMPLATE};

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
    /// `start_experience_factor` в процентах 0..=100 (нет поля — 0).
    pub experience_pct: u32,
    /// `start_equipment_factor` в процентах 0..=100 (нет поля — 100: полностью оснащена).
    pub equipment_pct: u32,
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
    experience_pct: Option<u32>,
    equipment_pct: Option<u32>,
}

/// Доля Paradox (`0.3`, `1`, `.25`) -> целые проценты 0..=100 без чисел с плавающей точкой.
/// Знаки после второго отбрасываются, больше 1.0 — 100. Мусор и отрицательные значения — `None`.
pub fn parse_factor_pct(text: &str) -> Option<u32> {
    let (int_part, frac_part) = text.split_once('.').unwrap_or((text, ""));
    if (int_part.is_empty() && frac_part.is_empty())
        || !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let whole: u32 = if int_part.is_empty() { 0 } else { int_part.parse().unwrap_or(u32::MAX) };
    if whole >= 1 {
        return Some(100);
    }
    let mut digits = frac_part.bytes().take(2).map(|b| u32::from(b - b'0'));
    let tens = digits.next().unwrap_or(0);
    let ones = digits.next().unwrap_or(0);
    Some(tens * 10 + ones)
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
            experience_pct: raw.experience_pct.unwrap_or(0),
            equipment_pct: raw.equipment_pct.unwrap_or(100),
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
                    ([u, d], "start_experience_factor") if u == "units" && d == "division" => {
                        current.experience_pct = parse_factor_pct(value)
                    }
                    ([u, d], "start_equipment_factor") if u == "units" && d == "division" => {
                        current.equipment_pct = parse_factor_pct(value)
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
mod real_oob_tests {
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

// ---------------------------------------------------------------------------
// Размещение и сопоставление шаблонов
// ---------------------------------------------------------------------------

/// Куда ставится дивизия из OOB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Как в файле.
    Exact(u32),
    /// Файловая провинция непригодна — дивизия переезжает в крупнейший город страны.
    Relocated { to: u32, reason: &'static str },
    /// Поставить некуда.
    Skipped(&'static str),
}

/// Крупнейший город страны: провинция с наибольшими победными очками в стейте страны с наибольшей
/// суммой очков (при равенстве — меньший id). `usable` отсеивает провинции без центра и море.
pub fn largest_city(registry: &StateRegistry, owner: &str, usable: &dyn Fn(u32) -> bool) -> Option<u32> {
    registry
        .states
        .iter()
        .filter(|s| s.owner == owner)
        .filter_map(|s| {
            let best = s
                .victory_points
                .iter()
                .filter(|&&(p, _)| usable(p))
                .max_by_key(|&&(p, vp)| (vp, std::cmp::Reverse(p)))?;
            let total: u32 = s.victory_points.iter().map(|&(_, vp)| vp).sum();
            Some((total, std::cmp::Reverse(s.id), best.0))
        })
        .max()
        .map(|(_, _, province)| province)
}

/// Определяет стартовую провинцию дивизии. Непригодная (нет на карте / море / чужой стейт) заменяется
/// на крупнейший город владельца; если и его нет — дивизия пропускается. Паник нет.
pub fn resolve_placement(entry: &OobDivision, registry: &StateRegistry, usable: &dyn Fn(u32) -> bool) -> Placement {
    let reason = if !usable(entry.location) {
        "провинции нет на карте или это море"
    } else if registry.state_of(entry.location).map(|s| s.owner.as_str()) != Some(entry.owner.as_str()) {
        "провинция не принадлежит стране"
    } else {
        return Placement::Exact(entry.location);
    };
    match largest_city(registry, &entry.owner, usable) {
        Some(to) => Placement::Relocated { to, reason },
        None => Placement::Skipped(reason),
    }
}

/// Шаблон из OOB -> зарегистрированный шаблон, доступный нации. Точное совпадение берётся как есть; иначе по
/// ключевым словам: бронетехника -> `Panzer-Division`, моторизованные и кавалерия -> `Infanterie-Division (mot.)`,
/// если нация их имеет; всё прочее (горные, колониальные, неизвестные) -> базовая пехота.
pub fn resolve_template(templates: &DivisionTemplates, owner: &str, requested: &str) -> &'static str {
    const PANZER: &str = "Panzer-Division";
    const MOT: &str = "Infanterie-Division (mot.)";
    if let Some(exact) = templates.available_names(owner).into_iter().find(|n| *n == requested) {
        return exact;
    }
    let lower = requested.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    let armored = has(&["panzer", "chars", "char ", "tank", "blind", "mécanique", "mecanique", "armo"]);
    let fast = has(&["mot", "cavaler", "kavaller", "cavalry", "schnell"]);
    if armored && templates.is_available(owner, PANZER) {
        PANZER
    } else if (armored || fast) && templates.is_available(owner, MOT) {
        MOT
    } else {
        BASE_TEMPLATE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::StateDef;
    use std::sync::Arc;

    const SAMPLE: &str = "\
units = {
    division = { # первая
        name = \"1. Infanterie-Division\"   # явное имя
        location = 6451 # Königsberg
        division_template = \"Infanterie-Division\"
        start_experience_factor = 0.3
        start_equipment_factor = 1.0
    }
    division =
    {
        division_name = { is_name_ordered = yes name_order = 25 }
        location   =   6334
        division_template =   \"Panzer-Division\"   # танки
    }
    division = { name = \"Без места\" division_template = \"Infanterie-Division\" }
    division = { location = abc name = \"Мусор\" }
}
";

    fn state(id: u32, owner: &str, vps: Vec<(u32, u32)>) -> Arc<StateDef> {
        Arc::new(StateDef {
            id,
            name: format!("S{id}"),
            owner: owner.into(),
            provinces: vps.iter().map(|&(p, _)| p).collect(),
            victory_points: vps,
            civilian_factories: 0,
            military_factories: 0,
            resources: Default::default(),
        })
    }

    fn registry() -> StateRegistry {
        let states = vec![
            state(1, "GER", vec![(10, 5), (11, 30)]),
            state(2, "GER", vec![(20, 40), (21, 1)]), // сумма 41 > 35 — главный город отсюда
            state(3, "FRA", vec![(30, 50)]),
        ];
        let mut registry = StateRegistry::default();
        for s in &states {
            for p in &s.provinces {
                registry.province_to_state.insert(*p, s.clone());
            }
        }
        registry.states = states;
        registry
    }

    fn entry(owner: &str, location: u32) -> OobDivision {
        OobDivision {
            name: "d".into(),
            owner: owner.into(),
            location,
            template: String::new(),
            experience_pct: 0,
            equipment_pct: 100,
        }
    }

    #[test]
    fn parser_survives_comments_odd_whitespace_and_garbage() {
        let (_, raws) = parse_oob(SAMPLE);
        assert_eq!(raws.len(), 4);
        assert_eq!(raws[0].name.as_deref(), Some("1. Infanterie-Division"));
        assert_eq!(raws[0].location, Some(6451));
        assert_eq!(raws[0].template.as_deref(), Some("Infanterie-Division"));
        assert_eq!((raws[1].location, raws[1].order), (Some(6334), Some(25)), "блок с переносами и лишними пробелами");
        assert_eq!(raws[1].template.as_deref(), Some("Panzer-Division"));
        assert_eq!(raws[2].location, None);
        assert_eq!(raws[3].location, None, "нечисловая провинция не паникует");
    }

    #[test]
    fn broken_scripts_never_panic() {
        for text in ["", "}", "{{{", "units = { division = { location =", "units = { division = { name = \"x", "= = ="] {
            let (_, raws) = parse_oob(text);
            let _ = raws.len();
        }
    }

    #[test]
    fn largest_city_is_the_top_vp_province_of_the_top_state() {
        let r = registry();
        assert_eq!(largest_city(&r, "GER", &|_| true), Some(20));
        assert_eq!(largest_city(&r, "GER", &|p| p != 20), Some(21), "недоступная провинция пропускается");
        assert_eq!(largest_city(&r, "ITA", &|_| true), None);
    }

    #[test]
    fn placement_keeps_valid_and_relocates_invalid_locations() {
        let r = registry();
        let usable = |p: u32| p != 11; // 11 — море / без центра
        assert_eq!(resolve_placement(&entry("GER", 10), &r, &usable), Placement::Exact(10));
        assert!(matches!(resolve_placement(&entry("GER", 11), &r, &usable), Placement::Relocated { to: 20, .. }), "нет центра");
        assert!(matches!(resolve_placement(&entry("GER", 999), &r, &|p| p != 999), Placement::Relocated { to: 20, .. }), "нет такой провинции");
        assert!(matches!(resolve_placement(&entry("GER", 30), &r, &|_| true), Placement::Relocated { to: 20, .. }), "чужая провинция");
        assert!(matches!(resolve_placement(&entry("ITA", 10), &r, &|_| true), Placement::Skipped(_)), "у страны нет городов");
    }

    #[test]
    fn templates_map_to_the_nearest_registered_one() {
        let t = DivisionTemplates::default();
        assert_eq!(resolve_template(&t, "GER", "Infanterie-Division"), "Infanterie-Division");
        assert_eq!(resolve_template(&t, "GER", "Panzer-Division"), "Panzer-Division");
        assert_eq!(resolve_template(&t, "GER", "Infanterie-Division (mot.)"), "Infanterie-Division (mot.)");
        assert_eq!(resolve_template(&t, "GER", "Kavallerie-Brigade"), "Infanterie-Division (mot.)");
        assert_eq!(resolve_template(&t, "GER", "Gebirgs-Brigade"), "Infanterie-Division");
        assert_eq!(resolve_template(&t, "GER", "Panzer-Brigade Neu"), "Panzer-Division");
        // у Франции танковой дивизии нет: бронетехника -> моторизованная, колониальные -> пехота
        assert_eq!(resolve_template(&t, "FRA", "Division Légère Mécanique"), "Infanterie-Division (mot.)");
        assert_eq!(resolve_template(&t, "FRA", "Brigade de Chars de Combat"), "Infanterie-Division (mot.)");
        assert_eq!(resolve_template(&t, "FRA", "Division d'Infanterie Motorisée"), "Infanterie-Division (mot.)");
        assert_eq!(resolve_template(&t, "FRA", "Division Coloniale"), "Infanterie-Division");
        assert_eq!(resolve_template(&t, "ITA", "Panzer-Division"), "Infanterie-Division", "чужая нация без шаблона");
        assert_eq!(resolve_template(&t, "GER", ""), "Infanterie-Division");
    }

    /// Реальный GER_1936: дивизии читаются, имена и провинции заполнены, шаблоны сопоставляются.
    #[test]
    fn real_data_ger_1936_is_parsed() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let divisions = load_country_oob(root, "GER").expect("GER_1936.txt");
        assert!(divisions.len() >= 20, "найдено {}", divisions.len());
        assert!(divisions.iter().all(|d| d.location > 0 && !d.name.is_empty() && d.owner == "GER"));
        assert!(divisions.iter().any(|d| d.name.contains("Infanterie-Division")));
        let templates = DivisionTemplates::default();
        for d in &divisions {
            let mapped = resolve_template(&templates, "GER", &d.template);
            assert!(templates.is_available("GER", mapped), "{} -> {mapped}", d.template);
        }
    }
}

#[cfg(test)]
mod start_factor_tests {
    use super::*;

    #[test]
    fn factors_become_integer_percent() {
        assert_eq!(parse_factor_pct("0.3"), Some(30));
        assert_eq!(parse_factor_pct("0.25"), Some(25));
        assert_eq!(parse_factor_pct(".5"), Some(50));
        assert_eq!(parse_factor_pct("0.999"), Some(99), "лишние знаки отбрасываются");
        assert_eq!(parse_factor_pct("1"), Some(100));
        assert_eq!(parse_factor_pct("1.0"), Some(100));
        assert_eq!(parse_factor_pct("2.5"), Some(100), "потолок 100%");
        assert_eq!(parse_factor_pct("0"), Some(0));
        assert_eq!(parse_factor_pct("-0.3"), None);
        assert_eq!(parse_factor_pct("abc"), None);
        assert_eq!(parse_factor_pct(""), None);
        assert_eq!(parse_factor_pct("."), None);
    }

    #[test]
    fn parser_reads_start_factors_and_applies_defaults() {
        let text = "units = {\n division = { # a\n  location = 1\n  division_template = \"Panzer-Division\"\n  start_experience_factor = 0.3\n  start_equipment_factor = 0.75\n }\n division = {\n  location = 2\n  division_template = \"Infanterie-Division\"\n }\n division = {\n  location = 3\n  start_experience_factor = oops\n  start_equipment_factor = 9\n }\n}";
        let (_, raws) = parse_oob(text);
        assert_eq!(raws.len(), 3);
        assert_eq!((raws[0].experience_pct, raws[0].equipment_pct), (Some(30), Some(75)));
        assert_eq!((raws[1].experience_pct, raws[1].equipment_pct), (None, None));
        assert_eq!((raws[2].experience_pct, raws[2].equipment_pct), (None, Some(100)), "мусор игнорируется, избыток режется");
    }

    #[test]
    fn real_data_has_start_experience_30_percent() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let divisions = load_country_oob(root, "GER").expect("GER_1936.txt");
        assert!(divisions.iter().all(|d| d.experience_pct == 30 && d.equipment_pct == 100));
    }
}
