//! Экономика (Фаза 5): фабрики стран, национальный склад и суточное производство.
//!
//! Всё считается целыми числами и привязано к тегу страны (`CountryEconomy`).
//! Тяжёлая логика идёт строго на `DailyTickEvent` (00:00), а не каждый час.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::clock::{ClockSet, DailyTickEvent};
use crate::division::is_water;
use crate::map::{ProvinceAdjacency, ProvinceControllers, ProvinceMap, StateRegistry};

/// Провинция с таким числом VP и больше — хаб снабжения.
const HUB_MIN_VICTORY_POINTS: u32 = 5;
/// Радиус действия хаба в шагах по графу смежности.
const SUPPLY_RADIUS: u32 = 3;

/// Стартовый запас винтовок на складе.
const START_INFANTRY_EQUIPMENT: i32 = 5000;
/// Винтовок в сутки с одного военного завода.
const RIFLES_PER_FACTORY_PER_DAY: i32 = 5;
const DEFAULT_PLAYER_COUNTRY: &str = "GER";

// ---------------------------------------------------------------------------
// Данные
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct CountryEconomy {
    pub civilian_factories: u32,
    pub military_factories: u32,
    pub infantry_equipment_stockpile: i32,
    /// Винтовок в сутки (пересчитывается из `military_factories`).
    pub daily_production: i32,
}

impl CountryEconomy {
    fn new(civilian_factories: u32, military_factories: u32) -> Self {
        let mut eco = Self {
            civilian_factories,
            military_factories,
            infantry_equipment_stockpile: START_INFANTRY_EQUIPMENT,
            daily_production: 0,
        };
        eco.recalc_daily_production();
        eco
    }

    fn recalc_daily_production(&mut self) {
        let factories = i32::try_from(self.military_factories).unwrap_or(i32::MAX);
        self.daily_production = factories.saturating_mul(RIFLES_PER_FACTORY_PER_DAY);
    }
}

/// Экономики всех стран по тегу. `BTreeMap` — детерминированный порядок обхода.
#[derive(Resource, Default)]
pub struct Economies(pub BTreeMap<String, CountryEconomy>);

/// Страна, за которую играет пользователь (её данные показывает верхняя панель).
#[derive(Resource)]
pub struct PlayerCountry(pub String);

/// Сеть снабжения: хабы и зона покрытия.
/// Хаб снабжает нацию, которая контролирует его провинцию сейчас: волна идёт по графу смежности
/// на `SUPPLY_RADIUS` шагов только по суше под контролем той же нации, провинции противника
/// и нейтралов её блокируют. Пересчитывается раз в сутки (`update_supply_network`).
#[derive(Resource, Default, Debug)]
pub struct SupplyNetwork {
    /// Провинции-кандидаты в хабы (столицы / центры стейтов с VP >= 5), по возрастанию id.
    pub hubs: BTreeSet<u32>,
    /// Покрытая снабжением провинция -> нация, которую снабжает хаб.
    pub coverage: BTreeMap<u32, String>,
}

impl SupplyNetwork {
    /// Снабжается ли провинция хабом указанной нации.
    pub fn supplies(&self, province: u32, country: &str) -> bool {
        self.coverage.get(&province).is_some_and(|c| c == country)
    }
}

/// Хабы по данным стейтов: провинции с победными очками не меньше порога.
fn hubs_from_registry(registry: &StateRegistry) -> BTreeSet<u32> {
    registry
        .states
        .iter()
        .flat_map(|s| s.victory_points.iter())
        .filter(|&&(_, points)| points >= HUB_MIN_VICTORY_POINTS)
        .map(|&(province, _)| province)
        .collect()
}

/// Зона покрытия всех хабов (чистая функция — удобно тестировать).
/// Каждый хаб обходится в ширину до `SUPPLY_RADIUS` шагов; порядок хабов и соседей
/// детерминирован (`BTreeSet`).
pub fn compute_coverage(
    hubs: &BTreeSet<u32>,
    controllers: &ProvinceControllers,
    adjacency: &ProvinceAdjacency,
    map: &ProvinceMap,
) -> BTreeMap<u32, String> {
    let mut coverage = BTreeMap::new();
    for &hub in hubs {
        let Some(nation) = controllers.0.get(&hub) else { continue };
        if is_water(map, hub) {
            continue;
        }
        let mut visited = BTreeSet::from([hub]);
        let mut frontier = vec![hub];
        coverage.insert(hub, nation.clone());
        for _ in 0..SUPPLY_RADIUS {
            let mut next = Vec::new();
            for &province in &frontier {
                for &n in adjacency.neighbors.get(&province).into_iter().flatten() {
                    // Чужие и нейтральные провинции и вода блокируют распространение волны
                    if visited.contains(&n) || is_water(map, n) || controllers.0.get(&n) != Some(nation) {
                        continue;
                    }
                    visited.insert(n);
                    coverage.insert(n, nation.clone());
                    next.push(n);
                }
            }
            frontier = next;
        }
    }
    coverage
}

// ---------------------------------------------------------------------------
// Плагин
// ---------------------------------------------------------------------------

pub struct EconomyPlugin;

impl Plugin for EconomyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Economies>()
            .init_resource::<SupplyNetwork>()
            .insert_resource(PlayerCountry(DEFAULT_PLAYER_COUNTRY.to_string()))
            .add_systems(Startup, (init_economies, init_supply_network))
            .add_systems(Update, (daily_production.after(ClockSet), update_supply_network.after(ClockSet), economy_bar));
    }
}

// ---------------------------------------------------------------------------
// Системы
// ---------------------------------------------------------------------------

/// Хабы снабжения на старте. Зона покрытия считается на первом суточном тике:
/// граф смежности создаётся в `setup_map` позже, а до тех пор все дивизии считаются снабжёнными.
fn init_supply_network(registry: Res<StateRegistry>, mut network: ResMut<SupplyNetwork>) {
    network.hubs = hubs_from_registry(&registry);
    println!("[СНАБЖЕНИЕ] Хабов снабжения: {} (VP >= {HUB_MIN_VICTORY_POINTS}), радиус {SUPPLY_RADIUS}", network.hubs.len());
}

/// Раз в сутки (00:00): пересчёт зоны покрытия по текущим контроллерам провинций.
/// Публична, чтобы статус снабжения дивизий (`division::daily_supply`) шёл строго после неё.
pub fn update_supply_network(
    mut events: EventReader<DailyTickEvent>,
    mut network: ResMut<SupplyNetwork>,
    controllers: Res<ProvinceControllers>,
    adjacency: Res<ProvinceAdjacency>,
    map: Res<ProvinceMap>,
) {
    if events.read().count() == 0 {
        return;
    }
    network.coverage = compute_coverage(&network.hubs, &controllers, &adjacency, &map);
}

/// Стартовая экономика: суммы фабрик из стейтов по владельцам.
fn init_economies(registry: Res<StateRegistry>, mut economies: ResMut<Economies>) {
    let mut total_civ = 0u64;
    let mut total_mil = 0u64;
    for (tag, (civ, mil)) in registry.factories_by_owner() {
        total_civ += u64::from(civ);
        total_mil += u64::from(mil);
        economies.0.insert(tag, CountryEconomy::new(civ, mil));
    }
    println!(
        "[ЭКОНОМИКА] Стран: {}, гражданских фабрик: {total_civ}, военных заводов: {total_mil}",
        economies.0.len()
    );
}

/// Раз в сутки (00:00) каждый военный завод добавляет винтовки на склад своей страны.
fn daily_production(mut events: EventReader<DailyTickEvent>, mut economies: ResMut<Economies>) {
    for event in events.read() {
        for eco in economies.0.values_mut() {
            eco.recalc_daily_production();
            eco.infantry_equipment_stockpile =
                eco.infantry_equipment_stockpile.saturating_add(eco.daily_production);
        }
        println!("[ЭКОНОМИКА] Сутки {}.{}.{}: производство учтено", event.day, event.month, event.year);
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// Верхняя плашка: фабрики и склад страны игрока.
fn economy_bar(mut contexts: EguiContexts, economies: Res<Economies>, player: Res<PlayerCountry>) {
    let ctx = contexts.ctx_mut();
    let frame = egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 200));

    egui::Window::new("economy_bar")
        .title_bar(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 10.0])
        .resizable(false)
        .frame(frame)
        .show(ctx, |ui| match economies.0.get(&player.0) {
            Some(eco) => {
                ui.label(format!(
                    "[{}] 🏭 Фабрики: {} | 🔨 Военные заводы: {} | 📦 Винтовки: {} (+{}/день)",
                    player.0,
                    eco.civilian_factories,
                    eco.military_factories,
                    eco.infantry_equipment_stockpile,
                    eco.daily_production
                ));
            }
            None => {
                ui.label(format!("[{}] нет данных об экономике", player.0));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::map::StateDef;

    /// Цепочка 1-2-3-4-5-6.
    fn chain() -> ProvinceAdjacency {
        let mut neighbors: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
        for a in 1..6u32 {
            neighbors.entry(a).or_default().insert(a + 1);
            neighbors.entry(a + 1).or_default().insert(a);
        }
        ProvinceAdjacency { neighbors }
    }

    fn controllers(owners: &[(u32, &str)]) -> ProvinceControllers {
        ProvinceControllers(owners.iter().map(|&(p, o)| (p, o.to_string())).collect())
    }

    fn covered(coverage: &BTreeMap<u32, String>) -> Vec<u32> {
        coverage.keys().copied().collect()
    }

    #[test]
    fn hub_supplies_three_steps_along_own_territory() {
        let all_ger = controllers(&[(1, "GER"), (2, "GER"), (3, "GER"), (4, "GER"), (5, "GER"), (6, "GER")]);
        let coverage = compute_coverage(&BTreeSet::from([1]), &all_ger, &chain(), &ProvinceMap::empty_for_test());
        // сам хаб и 3 шага: 2, 3, 4; провинции 5 и 6 вне радиуса
        assert_eq!(covered(&coverage), vec![1, 2, 3, 4]);
        assert!(coverage.values().all(|c| c == "GER"));
    }

    #[test]
    fn enemy_or_neutral_province_blocks_the_wave() {
        let map = ProvinceMap::empty_for_test();
        for blocker in ["FRA", "ITA"] {
            let owners = controllers(&[(1, "GER"), (2, "GER"), (3, blocker), (4, "GER"), (5, "GER"), (6, "GER")]);
            let coverage = compute_coverage(&BTreeSet::from([1]), &owners, &chain(), &map);
            assert_eq!(covered(&coverage), vec![1, 2], "{blocker} в провинции 3 режет волну за ней");
        }
        // провинция без контроллера (нейтральная земля) тоже стена
        let partial = controllers(&[(1, "GER"), (2, "GER"), (4, "GER")]);
        let coverage = compute_coverage(&BTreeSet::from([1]), &partial, &chain(), &map);
        assert_eq!(covered(&coverage), vec![1, 2]);
    }

    #[test]
    fn water_blocks_the_wave() {
        let map = ProvinceMap::with_terrain_for_test(&[(3, "ocean")]);
        let all_ger = controllers(&[(1, "GER"), (2, "GER"), (3, "GER"), (4, "GER")]);
        let coverage = compute_coverage(&BTreeSet::from([1]), &all_ger, &chain(), &map);
        assert_eq!(covered(&coverage), vec![1, 2]);
    }

    #[test]
    fn hub_supplies_whoever_controls_it_now() {
        // хаб в 1 захвачен Францией: теперь он снабжает французские провинции, а не германские
        let owners = controllers(&[(1, "FRA"), (2, "FRA"), (3, "GER")]);
        let network = SupplyNetwork {
            hubs: BTreeSet::from([1]),
            coverage: compute_coverage(&BTreeSet::from([1]), &owners, &chain(), &ProvinceMap::empty_for_test()),
        };
        assert!(network.supplies(1, "FRA") && network.supplies(2, "FRA"));
        assert!(!network.supplies(2, "GER"), "германская дивизия во французской зоне не снабжается");
        assert!(!network.supplies(3, "GER"), "провинция 3 вне зоны: волна идёт только по земле контроллера хаба");
    }

    #[test]
    fn two_hubs_cover_each_others_gaps() {
        let all_ger = controllers(&[(1, "GER"), (2, "GER"), (3, "GER"), (4, "GER"), (5, "GER"), (6, "GER")]);
        let coverage = compute_coverage(&BTreeSet::from([1, 6]), &all_ger, &chain(), &ProvinceMap::empty_for_test());
        assert_eq!(covered(&coverage), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn hubs_are_provinces_with_enough_victory_points() {
        let state = |id, vps: Vec<(u32, u32)>| {
            Arc::new(StateDef {
                id,
                name: format!("S{id}"),
                owner: "GER".into(),
                provinces: vec![],
                victory_points: vps,
                civilian_factories: 0,
                military_factories: 0,
            })
        };
        let registry = StateRegistry {
            states: vec![state(1, vec![(10, 50), (11, 5)]), state(2, vec![(20, 4), (21, 1)])],
            ..Default::default()
        };
        assert_eq!(hubs_from_registry(&registry), BTreeSet::from([10, 11]));
    }

    /// Проверка на реальных данных игры (пропускается без неё): сколько дивизий OOB 1936 попадает в зону хабов.
    #[test]
    fn real_data_start_coverage() {
        use std::path::Path;
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let registry = crate::map::load_all_states(&root.join("history").join("states")).expect("стейты");
        let map = crate::map::load_province_map(root).expect("карта");
        let (_, adjacency) = crate::map::build_id_texture_and_adjacency(&map);
        let controllers = ProvinceControllers(
            registry.province_to_state.iter().map(|(&p, s)| (p, s.owner.clone())).collect(),
        );
        let hubs = hubs_from_registry(&registry);
        let coverage = compute_coverage(&hubs, &controllers, &adjacency, &map);
        println!("хабов: {}, покрыто провинций: {}", hubs.len(), coverage.len());

        let oob = crate::oob::load_oob(root);
        for tag in ["GER", "FRA"] {
            let all: Vec<_> = oob.0.iter().filter(|d| d.owner == tag).collect();
            let supplied = all.iter().filter(|d| coverage.get(&d.location).is_some_and(|c| c == tag)).count();
            println!("{tag}: дивизий в зоне снабжения {supplied} из {}", all.len());
        }
        assert!(!hubs.is_empty() && !coverage.is_empty());
    }
}
