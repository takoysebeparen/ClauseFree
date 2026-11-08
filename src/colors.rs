//! Аутентичные цвета стран из файлов игры.
//!
//! Источники (читаются на лету из установленной игры):
//! 1. `common/country_tags/*.txt` -> путь к файлу страны -> `color = { r g b }` в этом файле;
//! 2. `common/countries/colors.txt` — приоритетный источник игры: `TAG = { color = rgb { .. } color_ui = rgb { .. } }`,
//!    цвет может быть задан как `rgb`, как голый `{ r g b }` или как `HSV { h s v }` (доли от 0 до 1).
//!
//! Разбор целочисленный: дроби читаются как тысячные доли, HSV переводится в RGB без `f32`.
//! Если для тега ничего не нашлось, берётся нейтральный серый — паник нет.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::prelude::*;
use bevy_egui::egui;

use crate::map::tokenize;

/// Нейтральный серый для тегов без цвета.
pub const FALLBACK_COLOR: [u8; 3] = [128, 128, 128];

/// Цвет страны: `map` — для раскраски карты, `ui` — более светлый цвет интерфейса (`color_ui`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountryColor {
    pub map: [u8; 3],
    pub ui: [u8; 3],
}

/// Цвета всех стран по тегу. `BTreeMap` — детерминированный порядок.
#[derive(Resource, Debug, Clone, Default)]
pub struct CountryColors(pub BTreeMap<String, CountryColor>);

impl CountryColors {
    /// Цвет страны на карте.
    pub fn map_rgb(&self, tag: &str) -> [u8; 3] {
        self.0.get(tag).map_or(FALLBACK_COLOR, |c| c.map)
    }

    /// Цвет страны в интерфейсе.
    pub fn ui_rgb(&self, tag: &str) -> [u8; 3] {
        self.0.get(tag).map_or(FALLBACK_COLOR, |c| c.ui)
    }

    /// Цвет интерфейса в формате egui.
    pub fn ui_color32(&self, tag: &str) -> egui::Color32 {
        let [r, g, b] = self.ui_rgb(tag);
        egui::Color32::from_rgb(r, g, b)
    }
}

// ---------------------------------------------------------------------------
// Разбор значений
// ---------------------------------------------------------------------------

/// Десятичная запись в тысячных долях без чисел с плавающей точкой: `0.15` -> 150, `.5` -> 500, `12` -> 12000.
/// Лишние знаки после третьего отбрасываются. Отрицательные и нечисловые записи — `None`.
fn parse_milli(text: &str) -> Option<u32> {
    let (int_part, frac_part) = text.split_once('.').unwrap_or((text, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if !int_part.chars().all(|c| c.is_ascii_digit()) || !frac_part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let int_value: u32 = if int_part.is_empty() { 0 } else { int_part.parse().ok()? };
    let mut frac = frac_part.chars().take(3).collect::<String>();
    while frac.len() < 3 {
        frac.push('0');
    }
    int_value.checked_mul(1000)?.checked_add(frac.parse().ok()?)
}

/// Тысячные доли (0..=1000) -> байт 0..=255 с округлением.
fn milli_to_byte(milli: u32) -> u8 {
    ((milli.min(1000) * 255 + 500) / 1000) as u8
}

/// HSV (доли от 0 до 1 в тысячных) -> RGB целочисленно. Оттенок 1.0 равен 0.0 (круг замкнут).
pub fn hsv_to_rgb(h_milli: u32, s_milli: u32, v_milli: u32) -> [u8; 3] {
    let (s, v) = (s_milli.min(1000), v_milli.min(1000));
    let h6 = (h_milli.min(1000) % 1000) * 6; // 0..=5999: шесть секторов по 1000
    let (sector, f) = (h6 / 1000, h6 % 1000);
    let p = v * (1000 - s) / 1000;
    let q = v * (1000 - s * f / 1000) / 1000;
    let t = v * (1000 - s * (1000 - f) / 1000) / 1000;
    let (r, g, b) = match sector {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [milli_to_byte(r), milli_to_byte(g), milli_to_byte(b)]
}

/// Разбирает значение цвета, начиная с токена `tokens[i]` (сразу после `color =`):
/// `rgb { r g b }`, `{ r g b }` или `hsv { h s v }` (регистр не важен).
/// Возвращает цвет и индекс токена после закрывающей скобки.
fn parse_color_value(tokens: &[String], mut i: usize) -> Option<([u8; 3], usize)> {
    let is_hsv = match tokens.get(i)?.to_ascii_lowercase().as_str() {
        "rgb" => {
            i += 1;
            false
        }
        "hsv" => {
            i += 1;
            true
        }
        "{" => false,
        _ => return None,
    };
    if tokens.get(i)? != "{" {
        return None;
    }
    let numbers: Vec<u32> = tokens[i + 1..].iter().take(3).map(|t| parse_milli(t)).collect::<Option<_>>()?;
    let [a, b, c] = numbers[..] else { return None };
    let rgb = if is_hsv {
        hsv_to_rgb(a, b, c)
    } else {
        // RGB: целые 0..=255 (допустима и дробная запись — округляется)
        let byte = |milli: u32| ((milli + 500) / 1000).min(255) as u8;
        [byte(a), byte(b), byte(c)]
    };
    // дальше должна стоять закрывающая скобка; лишние числа до неё пропускаем
    let close = tokens[i..].iter().position(|t| t == "}")? + i;
    Some((rgb, close + 1))
}

/// `common/country_tags/*.txt`: строки вида `GER = "countries/Germany.txt"`.
fn parse_tag_paths(text: &str) -> Vec<(String, String)> {
    let tokens = tokenize(text.trim_start_matches('\u{feff}'));
    let mut out = Vec::new();
    let mut i = 0;
    while i + 2 < tokens.len() {
        if tokens[i + 1] == "=" && !matches!(tokens[i].as_str(), "{" | "}" | "=") && tokens[i + 2].contains('/') {
            out.push((tokens[i].clone(), tokens[i + 2].clone()));
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

/// Цвет из файла страны: первое `color = ...` на верхнем уровне.
fn parse_country_file_color(text: &str) -> Option<[u8; 3]> {
    let tokens = tokenize(text.trim_start_matches('\u{feff}'));
    let mut depth = 0usize;
    for i in 0..tokens.len() {
        match tokens[i].as_str() {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            "color" if depth == 0 && tokens.get(i + 1).is_some_and(|t| t == "=") => {
                return parse_color_value(&tokens, i + 2).map(|(rgb, _)| rgb);
            }
            _ => {}
        }
    }
    None
}

/// `common/countries/colors.txt`: тег -> (`color`, `color_ui`), каждый необязателен.
fn parse_colors_file(text: &str) -> BTreeMap<String, (Option<[u8; 3]>, Option<[u8; 3]>)> {
    let tokens = tokenize(text.trim_start_matches('\u{feff}'));
    let mut out: BTreeMap<String, (Option<[u8; 3]>, Option<[u8; 3]>)> = BTreeMap::new();
    let mut tag: Option<String> = None;
    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i].as_str();
        match (&tag, token) {
            (None, name) if tokens.get(i + 1).is_some_and(|t| t == "=") && tokens.get(i + 2).is_some_and(|t| t == "{") => {
                tag = Some(name.to_string());
                out.entry(name.to_string()).or_default();
                i += 3;
            }
            (Some(_), "}") => {
                tag = None;
                i += 1;
            }
            (Some(current), key @ ("color" | "color_ui")) if tokens.get(i + 1).is_some_and(|t| t == "=") => {
                match parse_color_value(&tokens, i + 2) {
                    Some((rgb, next)) => {
                        let entry = out.entry(current.clone()).or_default();
                        if key == "color" {
                            entry.0 = Some(rgb);
                        } else {
                            entry.1 = Some(rgb);
                        }
                        i = next;
                    }
                    // битое значение: цвет остаётся не заданным, а блок значения пропускается до его скобки,
                    // чтобы закрывающая скобка не оборвала описание страны
                    None => {
                        i = tokens[i + 2..].iter().position(|t| t == "}").map_or(tokens.len(), |p| i + 2 + p + 1);
                    }
                }
            }
            _ => i += 1,
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Загрузка
// ---------------------------------------------------------------------------

fn read_text(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Читает цвета стран из `root` (папка игры). Любая недоступная часть пропускается без паники.
pub fn load_country_colors(root: &Path) -> CountryColors {
    let common = root.join("common");
    let mut base: BTreeMap<String, [u8; 3]> = BTreeMap::new();

    // 1. Файлы стран по тегам (порядок файлов фиксирован сортировкой)
    let mut tag_files: Vec<_> = std::fs::read_dir(common.join("country_tags"))
        .map(|dir| dir.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    tag_files.sort();
    for file in tag_files {
        let Some(text) = read_text(&file) else { continue };
        for (tag, relative) in parse_tag_paths(&text) {
            if let Some(rgb) = read_text(&common.join(relative)).and_then(|t| parse_country_file_color(&t)) {
                base.insert(tag, rgb);
            }
        }
    }
    let from_files = base.len();

    // 2. colors.txt переопределяет цвет и задаёт цвет интерфейса
    let overrides = read_text(&common.join("countries").join("colors.txt"))
        .map(|t| parse_colors_file(&t))
        .unwrap_or_default();
    let from_colors_file = overrides.values().filter(|(color, _)| color.is_some()).count();

    let mut colors = BTreeMap::new();
    let tags: std::collections::BTreeSet<&String> = base.keys().chain(overrides.keys()).collect();
    for tag in tags {
        let (override_map, override_ui) = overrides.get(tag).copied().unwrap_or((None, None));
        let Some(map) = override_map.or_else(|| base.get(tag).copied()) else { continue };
        colors.insert(tag.clone(), CountryColor { map, ui: override_ui.unwrap_or(map) });
    }
    println!(
        "[ЦВЕТА] Цветов стран: {} (из файлов стран: {from_files}, из colors.txt: {from_colors_file})",
        colors.len()
    );
    CountryColors(colors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<String> {
        tokenize(text)
    }

    #[test]
    fn decimals_are_read_as_thousandths_without_floats() {
        assert_eq!(parse_milli("0.15"), Some(150));
        assert_eq!(parse_milli(".5"), Some(500));
        assert_eq!(parse_milli("1"), Some(1000));
        assert_eq!(parse_milli("138"), Some(138_000));
        assert_eq!(parse_milli("0.12345"), Some(123), "лишние знаки отбрасываются");
        assert_eq!(parse_milli("1."), Some(1000));
        for bad in ["", ".", "-1", "abc", "1.2.3", "0,5"] {
            assert_eq!(parse_milli(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn hsv_converts_to_rgb_with_integers() {
        assert_eq!(hsv_to_rgb(0, 1000, 1000), [255, 0, 0], "красный");
        assert_eq!(hsv_to_rgb(500, 1000, 1000), [0, 255, 255], "голубой");
        assert_eq!(hsv_to_rgb(0, 0, 500), [128, 128, 128], "серый при нулевой насыщенности");
        assert_eq!(hsv_to_rgb(0, 0, 0), [0, 0, 0]);
        assert_eq!(hsv_to_rgb(1000, 1000, 1000), [255, 0, 0], "оттенок 1.0 замыкает круг");
        // HSV { 0.1 0.15 0.4 } — цвет Германии в colors.txt
        assert_eq!(hsv_to_rgb(100, 150, 400), [102, 96, 87]);
        // границы секторов: жёлто-зелёный (h = 0.167) и синий (h = 0.667) с учётом округления
        assert_eq!(hsv_to_rgb(167, 1000, 1000), [254, 255, 0]);
        assert_eq!(hsv_to_rgb(667, 1000, 1000), [1, 0, 255]);
    }

    #[test]
    fn color_value_forms() {
        let rgb = |s: &str| parse_color_value(&tokens(s), 0).map(|(c, _)| c);
        assert_eq!(rgb("rgb { 201 56 93 }"), Some([201, 56, 93]));
        assert_eq!(rgb("RGB{201 56 93}"), Some([201, 56, 93]));
        assert_eq!(rgb("{ 106  100  95 }"), Some([106, 100, 95]));
        assert_eq!(rgb("HSV { 0.1 0.15 0.4 }"), Some([102, 96, 87]));
        assert_eq!(rgb("hsv { 0 1 1 }"), Some([255, 0, 0]));
        assert_eq!(rgb("rgb { 300 56 93 }"), Some([255, 56, 93]), "значения выше 255 обрезаются");
        // битые значения не паникуют
        for bad in ["", "rgb", "rgb { 1 2 }", "rgb { a b c }", "foo { 1 2 3 }", "{ 1 2"] {
            assert_eq!(rgb(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn colors_file_with_rgb_hsv_and_odd_spacing() {
        let text = "\u{feff}#reload countrycolors\n\
            GER = {\n\tcolor = HSV { 0.1 0.15 0.4 }\n\tcolor_ui = rgb { 138 155 116 }\n}\n\
            ENG = {\n\tcolor = rgb { 201 56 93 }\n\tcolor_ui = rgb { 255 73 121 }\n}\n\
            BEL = { color \t = rgb { 193 171 8  }\n\t\tcolor_ui = rgb { 251 222 10 }\n\t  }\n\
            BAD = { color = rgb { x y z } color_ui = rgb { 1 2 3 } }\n";
        let parsed = parse_colors_file(text);
        assert_eq!(parsed["GER"], (Some([102, 96, 87]), Some([138, 155, 116])));
        assert_eq!(parsed["ENG"], (Some([201, 56, 93]), Some([255, 73, 121])));
        assert_eq!(parsed["BEL"], (Some([193, 171, 8]), Some([251, 222, 10])));
        assert_eq!(parsed["BAD"], (None, Some([1, 2, 3])), "битый color не мешает остальному");
    }

    #[test]
    fn tag_paths_and_country_file_color() {
        let tags = "GER\t= \"countries/Germany.txt\"\nENG = \"countries/United Kingdom.txt\"\n# комментарий\nSOV = \"countries/Soviet Union.txt\"";
        assert_eq!(
            parse_tag_paths(tags),
            vec![
                ("GER".to_string(), "countries/Germany.txt".to_string()),
                ("ENG".to_string(), "countries/United Kingdom.txt".to_string()),
                ("SOV".to_string(), "countries/Soviet Union.txt".to_string()),
            ]
        );
        let file = "graphical_culture = western_european_gfx\n\t \ncolor = { 106  119  89 } # { 76  97  121 }\n";
        assert_eq!(parse_country_file_color(file), Some([106, 119, 89]));
        assert_eq!(parse_country_file_color("graphical_culture = x"), None);
        // color внутри вложенного блока не считается цветом страны
        assert_eq!(parse_country_file_color("other = { color = { 1 2 3 } }"), None);
    }

    #[test]
    fn missing_tags_fall_back_to_neutral_gray() {
        let colors = CountryColors(BTreeMap::from([(
            "FRA".to_string(),
            CountryColor { map: [57, 113, 228], ui: [74, 147, 255] },
        )]));
        assert_eq!(colors.map_rgb("FRA"), [57, 113, 228]);
        assert_eq!(colors.ui_rgb("FRA"), [74, 147, 255]);
        assert_eq!(colors.map_rgb("XXX"), FALLBACK_COLOR);
        assert_eq!(colors.ui_rgb("XXX"), FALLBACK_COLOR);
        assert_eq!(colors.ui_color32("XXX"), egui::Color32::from_rgb(128, 128, 128));
        assert_eq!(CountryColors::default().map_rgb("GER"), FALLBACK_COLOR, "пустой ресурс не паникует");
    }

    #[test]
    fn loading_from_a_missing_folder_does_not_panic() {
        let colors = load_country_colors(Path::new("Z:\\такой\\папки\\нет"));
        assert!(colors.0.is_empty());
    }

    /// Цвета из реальных файлов игры (пропускается без игры).
    #[test]
    fn real_game_colors() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let colors = load_country_colors(root);
        println!("стран с цветом: {}", colors.0.len());
        assert!(colors.0.len() > 300, "в игре сотни стран");
        // colors.txt: GER в HSV, ENG / FRA в RGB, плюс цвета интерфейса
        assert_eq!(colors.map_rgb("GER"), [102, 96, 87]);
        assert_eq!(colors.ui_rgb("GER"), [138, 155, 116]);
        assert_eq!(colors.map_rgb("ENG"), [201, 56, 93]);
        assert_eq!(colors.map_rgb("FRA"), [57, 113, 228]);
        assert_eq!(colors.ui_rgb("FRA"), [74, 147, 255]);
        assert_eq!(colors.map_rgb("SOV"), [125, 13, 24]);
        // ни одной «случайной» замены: ключевые теги не серые
        for tag in ["GER", "FRA", "ENG", "SOV", "ITA", "POL", "USA", "JAP"] {
            assert_ne!(colors.map_rgb(tag), FALLBACK_COLOR, "{tag}");
        }
    }
}
