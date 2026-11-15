//! Воздушное превосходство по стратегическим регионам игры (`map/strategicregions/*.txt`).
//!
//! `StrategicAirRegions` сопоставляет провинции регионам и хранит для каждого региона силу авиации стран
//! (`BTreeMap<тег, очки 0..=100>`). Ядро расчёта не знает конкретных стран: перевес стороны в регионе —
//! разность её очков и очков противника (`edge`, -100..=+100). Раз в сутки (`DailyTickEvent`) небо смещается
//! к стране, которая держит на земле больше провинций региона. Бой читает готовое число: сторона с перевесом
//! бьёт сильнее (до +10% к атаке), сторона под давлением хуже обороняется (до -15% к защите).
//! Всё целочисленное, порядок — `BTreeMap`.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::prelude::*;

use crate::clock::{ClockSet, DailyTickEvent};
use crate::map::{ProvinceControllers, StateRegistry};

/// Граница шкалы превосходства: -MAX_AIR..=MAX_AIR; очки авиации региона лежат в 0..=MAX_AIR.
pub const MAX_AIR: i32 = 100;
/// Бонус к атаке при полном превосходстве, %.
pub const AIR_ATTACK_BONUS_MAX_PCT: i32 = 10;
/// Штраф к защите под полным вражеским превосходством, %.
pub const AIR_DEFENSE_PENALTY_MAX_PCT: i32 = 15;
/// На сколько очков в сутки растёт авиация страны, державшей на земле больше провинций региона.
const AIR_DRIFT_PER_DAY: i32 = 1;

/// Сценарий 1936: стейт приграничья и стартовые очки авиации стран над его регионом. 51 — Moselland
/// (Рейнская область), 17 — Alsace-Lorraine. Это данные сценария, а не логики расчёта.
const STARTING_AIR: [(u32, [(&str, i32); 2]); 2] = [(51, [("GER", 40), ("FRA", 0)]), (17, [("GER", 40), ("FRA", 0)])];

/// Стратегический регион: имя и провинции.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategicRegion {
    pub id: u32,
    pub name: String,
    pub provinces: Vec<u32>,
}

/// Реестр стратегических воздушных регионов и состояния неба над ними.
#[derive(Resource, Default, Debug, Clone)]
pub struct StrategicAirRegions {
    /// Провинция -> id региона.
    pub region_of: BTreeMap<u32, u32>,
    /// Регион -> локализованное имя.
    pub names: BTreeMap<u32, String>,
    /// Регион -> провинции (по возрастанию id).
    pub provinces: BTreeMap<u32, Vec<u32>>,
    /// Регион -> страна -> очки авиации (0..=100). Нет записи — паритет.
    pub power: BTreeMap<u32, BTreeMap<String, i32>>,
}

impl StrategicAirRegions {
    pub fn insert(&mut self, region: StrategicRegion) {
        for &p in &region.provinces {
            self.region_of.insert(p, region.id);
        }
        self.provinces.insert(region.id, region.provinces);
        self.names.insert(region.id, region.name);
    }

    /// Регион провинции.
    pub fn region_id(&self, province: u32) -> Option<u32> {
        self.region_of.get(&province).copied()
    }

    /// Название региона провинции (для интерфейса).
    pub fn region_name(&self, province: u32) -> Option<&str> {
        self.region_id(province).and_then(|r| self.names.get(&r)).map(String::as_str)
    }

    /// Задаёт очки авиации страны в регионе (обрезаются до 0..=100).
    pub fn set_power(&mut self, region: u32, country: &str, value: i32) {
        self.power.entry(region).or_default().insert(country.to_string(), value.clamp(0, MAX_AIR));
    }

    /// Очки авиации страны в регионе.
    pub fn power_of(&self, region: u32, country: &str) -> i32 {
        self.power.get(&region).and_then(|m| m.get(country)).copied().unwrap_or(0)
    }

    /// Перевес `own` над `enemy` в регионе провинции, -100..=+100 (положительный — в пользу `own`).
    pub fn edge(&self, province: u32, own: &str, enemy: &str) -> i32 {
        match self.region_id(province) {
            Some(region) => (self.power_of(region, own) - self.power_of(region, enemy)).clamp(-MAX_AIR, MAX_AIR),
            None => 0,
        }
    }
}

/// Бонус к атаке стороны с перевесом в небе: до +10% (при перевесе 100).
pub fn air_attack_bonus_pct(edge: i32) -> u32 {
    (edge.clamp(0, MAX_AIR) * AIR_ATTACK_BONUS_MAX_PCT / MAX_AIR) as u32
}

/// Штраф к защите стороны под вражеским воздушным давлением: до -15% (при перевесе противника 100).
pub fn air_defense_penalty_pct(edge: i32) -> u32 {
    ((-edge).clamp(0, MAX_AIR) * AIR_DEFENSE_PENALTY_MAX_PCT / MAX_AIR) as u32
}

/// Название страны для интерфейса (тег, если названия нет).
pub fn country_title(tag: &str) -> String {
    match tag {
        "GER" => "Германия".to_string(),
        "FRA" => "Франция".to_string(),
        other => other.to_string(),
    }
}

/// Подпись неба для интерфейса: перевес наступающей стороны `edge` (отрицательный — за обороной).
pub fn air_status(edge: i32, attacker: &str, defender: &str) -> String {
    match edge.signum() {
        1 => format!("{} +{edge}", country_title(attacker)),
        -1 => format!("{} +{}", country_title(defender), -edge),
        _ => "паритет".to_string(),
    }
}

/// Строка неба для плашки боя: `Небо (Западная Германия): Германия +40`.
pub fn air_line(region: Option<&str>, edge: i32, attacker: &str, defender: &str) -> String {
    match region {
        Some(name) => format!("Небо ({name}): {}", air_status(edge, attacker, defender)),
        None => format!("Небо: {}", air_status(edge, attacker, defender)),
    }
}

// ---------------------------------------------------------------------------
// Разбор файлов игры
// ---------------------------------------------------------------------------

/// Разбирает один файл `strategic_region = { id = N name = "KEY" provinces = { ... } .. }`.
/// Возвращает (id, ключ имени, провинции). Нет `id` или провинций — `None`. Мусор не вызывает паник.
pub fn parse_strategic_region(text: &str) -> Option<(u32, String, Vec<u32>)> {
    let tokens = crate::map::tokenize(text);
    let mut stack: Vec<String> = Vec::new();
    let (mut id, mut name) = (None, String::new());
    let mut provinces: Vec<u32> = Vec::new();
    for i in 0..tokens.len() {
        match tokens[i].as_str() {
            "{" => stack.push(if i >= 2 && tokens[i - 1] == "=" { tokens[i - 2].clone() } else { String::new() }),
            "}" => {
                stack.pop();
            }
            "=" => {}
            token => {
                let in_provinces = matches!(stack.as_slice(), [root, list] if root == "strategic_region" && list == "provinces");
                if in_provinces {
                    if let Ok(p) = token.parse::<u32>() {
                        provinces.push(p);
                    }
                    continue;
                }
                if tokens.get(i + 1).map(String::as_str) != Some("=") {
                    continue;
                }
                let Some(value) = tokens.get(i + 2) else { continue };
                if matches!(stack.as_slice(), [root] if root == "strategic_region") {
                    match token {
                        "id" => id = value.parse::<u32>().ok(),
                        "name" => name = value.clone(),
                        _ => {}
                    }
                }
            }
        }
    }
    let id = id?;
    if provinces.is_empty() {
        return None;
    }
    provinces.sort_unstable();
    provinces.dedup();
    Some((id, name, provinces))
}

/// Имя из имени файла `7-Western Germany.txt` -> `Western Germany`.
fn name_from_file(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    Some(stem.split_once('-').map_or(stem, |(_, rest)| rest).to_string())
}

/// Читает все `map/strategicregions/*.txt`; имена берёт из русской (затем английской) локализации,
/// иначе из имени файла. Нет папки — пустой реестр (воздух тогда нейтрален).
pub fn load_strategic_regions(root: &Path) -> StrategicAirRegions {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(root.join("map").join("strategicregions"))
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "txt")).collect())
        .unwrap_or_default();
    files.sort();
    let mut loc = BTreeMap::new();
    for lang in ["english", "russian"] {
        let path = root.join("localisation").join(lang).join(format!("strategic_region_names_l_{lang}.yml"));
        if let Ok(bytes) = std::fs::read(path) {
            loc.extend(crate::command::parse_localisation(&String::from_utf8_lossy(&bytes)));
        }
    }
    let mut regions = StrategicAirRegions::default();
    for path in files {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Some((id, key, provinces)) = parse_strategic_region(String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}')) else {
            continue;
        };
        let name = loc.get(&key).cloned().or_else(|| name_from_file(&path)).unwrap_or_else(|| format!("Регион {id}"));
        regions.insert(StrategicRegion { id, name, provinces });
    }
    regions
}

// ---------------------------------------------------------------------------
// Плагин
// ---------------------------------------------------------------------------

pub struct AirPlugin;

impl Plugin for AirPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StrategicAirRegions>()
            .add_systems(Startup, init_air_map)
            .add_systems(Update, update_air_superiority.after(ClockSet));
    }
}

/// Стартовая активность ВВС над приграничными регионами: регион берётся из провинций стейта сценария.
fn init_air_map(registry: Res<StateRegistry>, mut air: ResMut<StrategicAirRegions>) {
    for (state_id, powers) in STARTING_AIR {
        let Some(state) = registry.states.iter().find(|s| s.id == state_id) else { continue };
        let Some(region) = state.provinces.iter().find_map(|&p| air.region_id(p)) else { continue };
        for (country, value) in powers {
            let current = air.power_of(region, country);
            air.set_power(region, country, current.max(value));
        }
    }
    println!("[ВОЗДУХ] Стратегических регионов: {}, с заданным небом: {}", air.names.len(), air.power.len());
}

/// Раз в сутки авиация страны, державшей на земле строго больше провинций региона, чем любая другая из
/// участвующих в региональной воздушной войне, растёт на `AIR_DRIFT_PER_DAY`; на потолке шкалы остальные
/// теряют очки. Очки остаются в 0..=100.
pub fn update_air_superiority(
    mut events: EventReader<DailyTickEvent>,
    mut air: ResMut<StrategicAirRegions>,
    controllers: Res<ProvinceControllers>,
) {
    for _ in events.read() {
        let regions: Vec<u32> = air.power.keys().copied().collect();
        for region in regions {
            let provinces = air.provinces.get(&region).cloned().unwrap_or_default();
            let Some(powers) = air.power.get_mut(&region) else { continue };
            // сколько провинций региона держит каждая страна, участвующая в воздушной войне
            let held: BTreeMap<String, usize> = powers
                .keys()
                .map(|c| (c.clone(), provinces.iter().filter(|p| controllers.0.get(p).is_some_and(|x| x == c)).count()))
                .collect();
            let best = held.values().copied().max().unwrap_or(0);
            let leaders: Vec<&String> = held.iter().filter(|&(_, &n)| n == best && best > 0).map(|(c, _)| c).collect();
            let [leader] = leaders.as_slice() else { continue }; // ничья или никого — без изменений
            let leader = (*leader).clone();
            let at_cap = powers.get(&leader).is_some_and(|&v| v >= MAX_AIR);
            if at_cap {
                for value in powers.iter_mut().filter(|(c, _)| **c != leader).map(|(_, v)| v) {
                    *value = (*value - AIR_DRIFT_PER_DAY).max(0);
                }
            } else if let Some(value) = powers.get_mut(&leader) {
                *value = (*value + AIR_DRIFT_PER_DAY).min(MAX_AIR);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGION_FILE: &str = "strategic_region={\n\tid=7\n\tname=\"STRATEGICREGION_7\"\n\tprovinces={\n\t\t10 11 12 \n\t}\n\tweather={\n\t\tperiod={\n\t\t\tbetween={ 0.0 30.0 }\n\t\t\ttemperature={ -6.0 12.0 }\n\t\t}\n\t}\n}";

    #[test]
    fn bonus_and_penalty_scale_up_to_their_caps() {
        assert_eq!(air_attack_bonus_pct(40), 4);
        assert_eq!(air_defense_penalty_pct(-40), 6);
        assert_eq!(air_attack_bonus_pct(100), 10);
        assert_eq!(air_defense_penalty_pct(-100), 15);
        assert_eq!(air_attack_bonus_pct(-60), 0);
        assert_eq!(air_defense_penalty_pct(60), 0);
        assert_eq!(air_attack_bonus_pct(500), 10);
        assert_eq!(air_defense_penalty_pct(-500), 15);
    }

    #[test]
    fn status_labels_include_the_region_name() {
        assert_eq!(air_status(40, "GER", "FRA"), "Германия +40");
        assert_eq!(air_status(-25, "GER", "FRA"), "Франция +25");
        assert_eq!(air_status(0, "GER", "FRA"), "паритет");
        assert_eq!(air_status(10, "ITA", "ENG"), "ITA +10", "страны без названия — тегом");
        assert_eq!(air_line(Some("Западная Германия"), 40, "GER", "FRA"), "Небо (Западная Германия): Германия +40");
        assert_eq!(air_line(None, 0, "GER", "FRA"), "Небо: паритет");
    }

    #[test]
    fn region_file_is_parsed_without_weather_noise() {
        let (id, name, provinces) = parse_strategic_region(REGION_FILE).expect("регион");
        assert_eq!((id, name.as_str(), provinces), (7, "STRATEGICREGION_7", vec![10, 11, 12]));
        assert!(parse_strategic_region("").is_none());
        assert!(parse_strategic_region("strategic_region={ id=1 }").is_none(), "без провинций");
        assert!(parse_strategic_region("strategic_region={ provinces={ 1 2 } }").is_none(), "без id");
        assert!(parse_strategic_region("}}} = { { provinces={ 5").is_none());
    }

    fn regions() -> StrategicAirRegions {
        let mut r = StrategicAirRegions::default();
        r.insert(StrategicRegion { id: 7, name: "Западная Германия".into(), provinces: vec![1, 2, 3] });
        r.insert(StrategicRegion { id: 9, name: "Другой".into(), provinces: vec![4] });
        r
    }

    #[test]
    fn provinces_map_to_regions_and_edge_is_symmetric() {
        let mut r = regions();
        assert_eq!((r.region_id(2), r.region_id(4), r.region_id(99)), (Some(7), Some(9), None));
        assert_eq!(r.region_name(3), Some("Западная Германия"));
        assert_eq!(r.region_name(99), None);
        r.set_power(7, "GER", 40);
        r.set_power(7, "FRA", 10);
        assert_eq!(r.edge(1, "GER", "FRA"), 30);
        assert_eq!(r.edge(1, "FRA", "GER"), -30);
        assert_eq!(r.edge(1, "ITA", "ENG"), 0, "посторонние страны — паритет");
        assert_eq!(r.edge(4, "GER", "FRA"), 0, "другой регион");
        assert_eq!(r.edge(99, "GER", "FRA"), 0, "провинция вне регионов");
        r.set_power(7, "GER", 500);
        assert_eq!(r.power_of(7, "GER"), 100, "очки обрезаются");
    }

    fn app_with(controlled_by: &[(u32, &str)], start: &[(&str, i32)]) -> App {
        let mut regions = regions();
        for &(country, value) in start {
            regions.set_power(7, country, value);
        }
        let mut app = App::new();
        app.add_event::<DailyTickEvent>()
            .insert_resource(regions)
            .insert_resource(ProvinceControllers(controlled_by.iter().map(|&(p, c)| (p, c.to_string())).collect()))
            .add_systems(Update, update_air_superiority);
        app
    }

    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    fn edge(app: &App) -> i32 {
        app.world().resource::<StrategicAirRegions>().edge(1, "GER", "FRA")
    }

    #[test]
    fn sky_drifts_toward_the_side_holding_the_ground() {
        let mut german = app_with(&[(1, "GER"), (2, "GER"), (3, "FRA")], &[("GER", 40), ("FRA", 0)]);
        day(&mut german);
        day(&mut german);
        assert_eq!(edge(&german), 42);

        let mut french = app_with(&[(1, "FRA"), (2, "FRA"), (3, "GER")], &[("GER", 40), ("FRA", 0)]);
        day(&mut french);
        assert_eq!(edge(&french), 39);

        // поровну — без изменений
        let mut split = app_with(&[(1, "FRA"), (2, "GER")], &[("GER", 40), ("FRA", 0)]);
        day(&mut split);
        assert_eq!(edge(&split), 40);
    }

    #[test]
    fn core_works_for_any_pair_of_countries() {
        let mut app = app_with(&[], &[]);
        app.insert_resource(ProvinceControllers([(1, "ITA".to_string()), (2, "ITA".to_string()), (3, "ENG".to_string())].into()));
        {
            let mut air = app.world_mut().resource_mut::<StrategicAirRegions>();
            air.set_power(7, "ITA", 20);
            air.set_power(7, "ENG", 20);
        }
        day(&mut app);
        let air = app.world().resource::<StrategicAirRegions>();
        assert_eq!((air.edge(1, "ITA", "ENG"), air.edge(1, "GER", "FRA")), (1, 0));
    }

    #[test]
    fn sky_stays_within_the_scale() {
        let mut high = app_with(&[(1, "GER"), (2, "GER"), (3, "GER")], &[("GER", 100), ("FRA", 0)]);
        day(&mut high);
        assert_eq!(edge(&high), 100);
        let mut low = app_with(&[(1, "FRA"), (2, "FRA"), (3, "FRA")], &[("GER", 0), ("FRA", 100)]);
        day(&mut low);
        assert_eq!(edge(&low), -100);
        // на потолке лидера проигравший теряет очки, но не уходит ниже нуля
        let mut cap = app_with(&[(1, "GER"), (2, "GER")], &[("GER", 100), ("FRA", 5)]);
        day(&mut cap);
        assert_eq!(cap.world().resource::<StrategicAirRegions>().power_of(7, "FRA"), 4);
    }

    /// Реальные strategicregions (пропускается без игры): Западная Германия, Берлин и приграничные стейты.
    #[test]
    fn real_data_regions_cover_the_start_scenario() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let regions = load_strategic_regions(root);
        assert!(regions.names.len() >= 250, "регионов {}", regions.names.len());
        assert_eq!(regions.names.get(&7).map(String::as_str), Some("Западная Германия"));
        assert_eq!(regions.region_name(9375), Some("Северо-Западная Германия"));
        // каждая провинция принадлежит ровно одному региону
        let total: usize = regions.provinces.values().map(Vec::len).sum();
        assert_eq!(total, regions.region_of.len(), "регионы не пересекаются");
        // стартовые стейты сценария лежат в реальных регионах
        let states = crate::map::load_all_states(&root.join("history").join("states")).expect("стейты");
        for (state_id, _) in STARTING_AIR {
            let state = states.states.iter().find(|s| s.id == state_id).expect("стейт");
            assert!(state.provinces.iter().any(|&p| regions.region_id(p).is_some()), "стейт {state_id}");
        }
    }

    #[test]
    fn missing_game_gives_neutral_sky() {
        let regions = load_strategic_regions(Path::new("Z:\\нет\\игры"));
        assert!(regions.names.is_empty());
        assert_eq!(regions.edge(1, "GER", "FRA"), 0);
    }
}
