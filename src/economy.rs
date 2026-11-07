//! Экономика (Фаза 5): фабрики стран, национальный склад и суточное производство.
//!
//! Всё считается целыми числами и привязано к тегу страны (`CountryEconomy`).
//! Тяжёлая логика идёт строго на `DailyTickEvent` (00:00), а не каждый час.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::clock::{ClockSet, DailyTickEvent};
use crate::division::{daily_supply, is_water, Division};
use crate::map::{ProvinceAdjacency, ProvinceControllers, ProvinceMap, StateRegistry};

/// Провинция с таким числом VP и больше — хаб снабжения.
const HUB_MIN_VICTORY_POINTS: u32 = 5;
/// Радиус действия хаба в шагах по графу смежности.
const SUPPLY_RADIUS: u32 = 3;

/// Стартовый запас винтовок на складе.
const START_INFANTRY_EQUIPMENT: i32 = 5000;
/// Максимум единиц прочности, запрашиваемых одной дивизией в сутки (1 единица = 1 винтовка).
const REINFORCE_PER_DIVISION_PER_DAY: u32 = 50;
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
    /// Винтовок, реально потраченных на пополнение дивизий в последние сутки.
    pub reinforce_cost: i32,
}

impl CountryEconomy {
    fn new(civilian_factories: u32, military_factories: u32) -> Self {
        let mut eco = Self {
            civilian_factories,
            military_factories,
            infantry_equipment_stockpile: START_INFANTRY_EQUIPMENT,
            daily_production: 0,
            reinforce_cost: 0,
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
            // Суточный порядок: сеть снабжения -> статус снабжения дивизий (division::daily_supply);
            // производство на склад; затем пополнение со склада — только снабжённым дивизиям.
            .add_systems(
                Update,
                (
                    daily_production.after(ClockSet),
                    update_supply_network.after(ClockSet),
                    daily_reinforcement.after(daily_production).after(daily_supply),
                    economy_bar,
                ),
            );
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

/// Суточное пополнение: дивизии с неполной прочностью запрашивают до 50 единиц, склад страны тратит
/// столько же винтовок. Пополняются ТОЛЬКО снабжённые дивизии (`is_supplied`): отрезанным склад
/// ничего не отправляет, их прочность не растёт, а в расход и запросы они не попадают.
/// При нехватке запас делится пропорционально запросам (целочисленно, остаток раздаётся по одному
/// в порядке id). Всё детерминировано: страны и дивизии — по порядку.
fn daily_reinforcement(
    mut events: EventReader<DailyTickEvent>,
    mut economies: ResMut<Economies>,
    mut divisions: Query<(Entity, &mut Division)>,
) {
    for _ in events.read() {
        for eco in economies.0.values_mut() {
            eco.reinforce_cost = 0;
        }

        // Запросы по владельцам: (id дивизии, сущность, запрошено снаряжения, запрошено прочности).
        // Стоимость пополнения берётся из шаблона: снаряжения на единицу прочности =
        // total_required_equipment / max_strength (для пехоты 1:1), округление вверх.
        let mut requests: BTreeMap<String, Vec<(u32, Entity, u32, u32)>> = BTreeMap::new();
        let mut cut_off: BTreeMap<String, u32> = BTreeMap::new();
        for (entity, d) in divisions.iter() {
            let deficit = d.max_strength.saturating_sub(d.current_strength);
            if deficit == 0 {
                continue;
            }
            if !d.is_supplied {
                *cut_off.entry(d.owner.clone()).or_default() += 1; // отрезана: пополнение не отправляется
                continue;
            }
            let strength_request = deficit.min(REINFORCE_PER_DIVISION_PER_DAY);
            let equipment_request = (u64::from(strength_request) * u64::from(d.required_equipment))
                .div_ceil(u64::from(d.max_strength.max(1))) as u32;
            requests.entry(d.owner.clone()).or_default().push((d.id, entity, equipment_request, strength_request));
        }

        for (tag, skipped) in &cut_off {
            println!("[СНАБЖЕНИЕ] {tag}: отрезанных дивизий без пополнения: {skipped}");
        }

        for (tag, mut list) in requests {
            let Some(eco) = economies.0.get_mut(&tag) else { continue };
            list.sort_unstable_by_key(|&(id, ..)| id);

            let available = u64::try_from(eco.infantry_equipment_stockpile.max(0)).unwrap_or(0);
            let total: u64 = list.iter().map(|&(_, _, r, _)| u64::from(r)).sum();

            let grants: Vec<u32> = if total <= available {
                list.iter().map(|&(_, _, r, _)| r).collect()
            } else {
                // Нехватка: доля = запрос * склад / сумма запросов (с округлением вниз)
                let mut g: Vec<u32> =
                    list.iter().map(|&(_, _, r, _)| (u64::from(r) * available / total) as u32).collect();
                let mut rest = available - g.iter().map(|&x| u64::from(x)).sum::<u64>();
                for (grant, &(_, _, r, _)) in g.iter_mut().zip(&list) {
                    if rest == 0 {
                        break;
                    }
                    if *grant < r {
                        *grant += 1;
                        rest -= 1;
                    }
                }
                g
            };

            let mut spent = 0u64;
            for (&(_, entity, request_eq, strength_request), &grant) in list.iter().zip(&grants) {
                // Прирост прочности пропорционален выданной доле снаряжения
                let gain = if request_eq == 0 || grant >= request_eq {
                    strength_request
                } else {
                    (u64::from(grant) * u64::from(strength_request) / u64::from(request_eq)) as u32
                };
                if gain == 0 {
                    continue;
                }
                if let Ok((_, mut d)) = divisions.get_mut(entity) {
                    d.current_strength = (d.current_strength + gain).min(d.max_strength);
                    spent += u64::from(grant);
                }
            }

            let spent_i32 = i32::try_from(spent).unwrap_or(i32::MAX);
            eco.infantry_equipment_stockpile = eco.infantry_equipment_stockpile.saturating_sub(spent_i32);
            eco.reinforce_cost = spent_i32;
            if spent > 0 {
                let note = if total > available { " (нехватка винтовок)" } else { "" };
                println!(
                    "[СНАБЖЕНИЕ] {tag}: пополнено дивизий: {}, винтовок: {spent}, остаток на складе: {}{note}",
                    grants.iter().filter(|&&g| g > 0).count(),
                    eco.infantry_equipment_stockpile
                );
            }
        }
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
                    "[{}] 🏭 Фабрики: {} | 🔨 Военные заводы: {} | 📦 Винтовки: {} (+{} / -{} день)",
                    player.0,
                    eco.civilian_factories,
                    eco.military_factories,
                    eco.infantry_equipment_stockpile,
                    eco.daily_production,
                    eco.reinforce_cost
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

#[cfg(test)]
mod reinforcement_tests {
    use super::*;
    use crate::units::{DivisionTemplates, BASE_TEMPLATE};

    fn spawn_division(app: &mut App, id: u32, province: u32, strength: u32) -> Entity {
        let templates = DivisionTemplates::default();
        let mut d = Division::from_template(
            id,
            format!("Div {id}"),
            "GER".to_string(),
            province,
            BASE_TEMPLATE.to_string(),
            templates.get_or_base(BASE_TEMPLATE),
        );
        d.current_strength = strength;
        app.world_mut().spawn(d).id()
    }

    fn set_supplied(app: &mut App, e: Entity, supplied: bool) {
        app.world_mut().get_mut::<Division>(e).unwrap().is_supplied = supplied;
    }

    fn strength(app: &App, e: Entity) -> u32 {
        app.world().get::<Division>(e).unwrap().current_strength
    }

    fn stock(app: &App) -> (i32, i32) {
        let eco = &app.world().resource::<Economies>().0["GER"];
        (eco.infantry_equipment_stockpile, eco.reinforce_cost)
    }

    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    /// Приложение с одной экономикой GER и заданным запасом винтовок.
    fn app_with_stock(stockpile: i32) -> App {
        let mut app = App::new();
        let mut eco = CountryEconomy::new(10, 10);
        eco.infantry_equipment_stockpile = stockpile;
        app.add_event::<DailyTickEvent>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), eco)])))
            .add_systems(Update, daily_reinforcement);
        app
    }

    #[test]
    fn supplied_division_is_reinforced_from_the_stockpile() {
        let mut app = app_with_stock(1000);
        let d = spawn_division(&mut app, 1, 1, 800);
        day(&mut app);
        // до 50 единиц прочности в сутки, по винтовке на единицу
        assert_eq!(strength(&app, d), 850);
        assert_eq!(stock(&app), (950, 50));
        // второй день: остаток дефицита 50 -> полностью укомплектована
        day(&mut app);
        assert_eq!(strength(&app, d), 900);
        assert_eq!(stock(&app), (900, 50));
        // полный состав больше ничего не просит
        day(&mut app);
        assert_eq!(stock(&app), (900, 0));
    }

    #[test]
    fn cut_off_division_gets_nothing_and_costs_nothing() {
        let mut app = app_with_stock(1000);
        let d = spawn_division(&mut app, 1, 1, 700);
        set_supplied(&mut app, d, false);
        let before = app.world().get::<Division>(d).unwrap().losses;
        day(&mut app);
        assert_eq!(strength(&app, d), 700, "отрезанной дивизии подкрепления не отправляются");
        assert_eq!(stock(&app), (1000, 0), "склад не тратится");
        assert_eq!(app.world().get::<Division>(d).unwrap().losses, before);

        // снабжение восстановилось — пополнение возобновляется
        set_supplied(&mut app, d, true);
        day(&mut app);
        assert_eq!(strength(&app, d), 750);
        assert_eq!(stock(&app), (950, 50));
    }

    #[test]
    fn cut_off_divisions_do_not_take_a_share_of_a_short_stockpile() {
        // Запас 60 при запросах 2 x 50 от снабжённых: поровну по 30; отрезанная третья не получает ничего
        let mut app = app_with_stock(60);
        let a = spawn_division(&mut app, 1, 1, 800);
        let b = spawn_division(&mut app, 2, 1, 800);
        let cut = spawn_division(&mut app, 3, 1, 800);
        set_supplied(&mut app, cut, false);
        day(&mut app);
        assert_eq!((strength(&app, a), strength(&app, b), strength(&app, cut)), (830, 830, 800));
        assert_eq!(stock(&app), (0, 60));
    }

    #[test]
    fn full_daily_chain_attrition_then_supply_check_then_reinforcement() {
        // Хаб в провинции 1 снабжает 1 и 2; провинция 9 далеко от любого хаба
        let mut app = App::new();
        let neighbors = BTreeMap::from([(1, BTreeSet::from([2])), (2, BTreeSet::from([1]))]);
        let mut eco = CountryEconomy::new(10, 10);
        eco.infantry_equipment_stockpile = 1000;
        app.add_event::<DailyTickEvent>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), eco)])))
            .insert_resource(SupplyNetwork { hubs: BTreeSet::from([1]), coverage: BTreeMap::new() })
            .insert_resource(ProvinceControllers(BTreeMap::from([
                (1, "GER".to_string()),
                (2, "GER".to_string()),
                (9, "GER".to_string()),
            ])))
            .insert_resource(ProvinceAdjacency { neighbors })
            .insert_resource(ProvinceMap::empty_for_test())
            .add_systems(
                Update,
                (
                    update_supply_network,
                    daily_supply.after(update_supply_network),
                    daily_reinforcement.after(daily_supply),
                ),
            );

        let connected = spawn_division(&mut app, 1, 2, 800);
        let isolated = spawn_division(&mut app, 2, 9, 800);

        day(&mut app);
        // подключённая: снабжена, +50; изолированная в тот же день отмечена как отрезанная,
        // теряет 2% (16) и подкрепления не получает
        assert_eq!(strength(&app, connected), 850);
        assert_eq!(strength(&app, isolated), 784);
        assert!(!app.world().get::<Division>(isolated).unwrap().is_supplied);
        assert_eq!(stock(&app), (950, 50));
    }
}
