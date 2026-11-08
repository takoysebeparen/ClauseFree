//! Экономика (Фаза 5): фабрики стран, многотиповой национальный склад, производственные линии
//! и адресное суточное пополнение дивизий.
//!
//! Всё считается целыми числами и привязано к тегу страны (`CountryEconomy`).
//! Тяжёлая логика идёт строго на `DailyTickEvent` (00:00), а не каждый час.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::clock::{ClockSet, DailyTickEvent};
use crate::colors::CountryColors;
use crate::division::{daily_supply, is_water, Division};
use crate::map::{ProvinceAdjacency, ProvinceControllers, ProvinceMap, StateRegistry};

/// Провинция с таким числом VP и больше — хаб снабжения.
const HUB_MIN_VICTORY_POINTS: u32 = 5;
/// Радиус действия хаба в шагах по графу смежности.
const SUPPLY_RADIUS: u32 = 3;

/// Максимум единиц прочности, запрашиваемых одной дивизией в сутки.
const REINFORCE_PER_DIVISION_PER_DAY: u32 = 50;
const DEFAULT_PLAYER_COUNTRY: &str = "GER";

/// Доля гражданских фабрик, занятых товарами народного потребления, %.
pub const CONSUMER_GOODS_SHARE_PCT: u32 = 20;
/// Стоимость военного завода в очках стройки.
pub const MILITARY_FACTORY_COST: u32 = 7200;
/// Очки прогресса, которые одна свободная гражданская фабрика даёт проекту в сутки.
pub const POINTS_PER_FACTORY_PER_DAY: u32 = 5;
/// Больше стольких фабрик на один проект не назначается: остаток уходит в следующий проект очереди.
pub const MAX_FACTORIES_PER_PROJECT: u32 = 15;

/// Доли военных заводов по линиям на старте, %: остаток от округления вниз достаётся пехотному снаряжению.
const START_LINE_SHARES: [(EquipmentType, u32); 5] = [
    (EquipmentType::InfantryEquipment, 50),
    (EquipmentType::SupportEquipment, 10),
    (EquipmentType::Artillery, 15),
    (EquipmentType::LightTank, 25),
    (EquipmentType::MediumTank, 0),
];

// ---------------------------------------------------------------------------
// Типы снаряжения
// ---------------------------------------------------------------------------

/// Вид военного снаряжения. Ключ многотипового склада и потребностей батальонов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EquipmentType {
    /// Пехотное снаряжение (винтовки).
    InfantryEquipment,
    /// Дополнительное оснащение сапёров и разведки.
    SupportEquipment,
    /// Орудия.
    Artillery,
    LightTank,
    MediumTank,
}

impl EquipmentType {
    /// Все типы в фиксированном порядке (детерминированный обход).
    pub const ALL: [EquipmentType; 5] = [
        EquipmentType::InfantryEquipment,
        EquipmentType::SupportEquipment,
        EquipmentType::Artillery,
        EquipmentType::LightTank,
        EquipmentType::MediumTank,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EquipmentType::InfantryEquipment => "Винтовки",
            EquipmentType::SupportEquipment => "Оснащение",
            EquipmentType::Artillery => "Пушки",
            EquipmentType::LightTank => "Лёгкие танки",
            EquipmentType::MediumTank => "Средние танки",
        }
    }

    /// Суточный выход одного военного завода: 5 винтовок, 1 орудие или 1 танк (оснащение — 3).
    pub fn output_per_factory(self) -> u32 {
        match self {
            EquipmentType::InfantryEquipment => 5,
            EquipmentType::SupportEquipment => 3,
            EquipmentType::Artillery | EquipmentType::LightTank | EquipmentType::MediumTank => 1,
        }
    }

    /// Стартовый запас каждой страны.
    fn start_stock(self) -> u32 {
        match self {
            EquipmentType::InfantryEquipment => 5000,
            EquipmentType::SupportEquipment => 500,
            EquipmentType::Artillery => 300,
            EquipmentType::LightTank => 600,
            EquipmentType::MediumTank => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Данные
// ---------------------------------------------------------------------------

/// Производственная линия: на сколько заводов назначено производство типа снаряжения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionLine {
    pub equipment: EquipmentType,
    pub assigned_factories: u32,
}

/// Тип здания, которое можно строить.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildingType {
    MilitaryFactory,
}

/// Проект стройки в стейте.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructionProject {
    pub state_id: u32,
    pub building_type: BuildingType,
    /// Накопленные очки стройки.
    pub progress: u32,
    /// Сколько очков нужно для завершения.
    pub cost: u32,
}

impl ConstructionProject {
    pub fn military_factory(state_id: u32) -> Self {
        Self { state_id, building_type: BuildingType::MilitaryFactory, progress: 0, cost: MILITARY_FACTORY_COST }
    }

    /// Сколько фабрик нужно, чтобы закончить проект за сутки (округление вверх).
    fn factories_to_finish(&self) -> u32 {
        self.cost.saturating_sub(self.progress).div_ceil(POINTS_PER_FACTORY_PER_DAY)
    }
}

/// Очередь стройки страны. Свободные гражданские фабрики работают над проектами по порядку.
#[derive(Debug, Clone, Default)]
pub struct ConstructionQueue {
    pub projects: Vec<ConstructionProject>,
}

impl ConstructionQueue {
    /// Сколько фабрик работает над каждым проектом очереди при vailable свободных: не больше 15
    /// на проект и не больше, чем нужно для завершения; остаток переходит на следующий проект.
    pub fn allocation(&self, available: u32) -> Vec<u32> {
        let mut remaining = available;
        self.projects
            .iter()
            .map(|project| {
                let assigned = remaining.min(MAX_FACTORIES_PER_PROJECT).min(project.factories_to_finish());
                remaining -= assigned;
                assigned
            })
            .collect()
    }

    /// Суточный прогресс: каждая фабрика даёт 5 очков своему проекту. Завершённые проекты
    /// (прогресс не меньше стоимости) удаляются из очереди и возвращаются в порядке очереди.
    pub fn advance(&mut self, available: u32) -> Vec<ConstructionProject> {
        let assigned = self.allocation(available);
        for (project, factories) in self.projects.iter_mut().zip(assigned) {
            project.progress = (project.progress + factories * POINTS_PER_FACTORY_PER_DAY).min(project.cost);
        }
        let (done, pending): (Vec<_>, Vec<_>) = self.projects.drain(..).partition(|p| p.progress >= p.cost);
        self.projects = pending;
        done
    }
}

#[derive(Debug, Clone, Default)]
pub struct CountryEconomy {
    pub civilian_factories: u32,
    pub military_factories: u32,
    /// Склад по типам снаряжения (нет записи — ноль).
    pub stockpile: BTreeMap<EquipmentType, u32>,
    /// Производственные линии; сумма назначенных заводов не превышает `military_factories`.
    pub production_lines: Vec<ProductionLine>,
    /// Суточный выход по типам (пересчитывается из линий).
    pub daily_output: BTreeMap<EquipmentType, u32>,
    /// Потрачено на пополнение дивизий за последние сутки, по типам.
    pub reinforce_cost: BTreeMap<EquipmentType, u32>,
    /// Очередь стройки военных заводов.
    pub construction: ConstructionQueue,
}

impl CountryEconomy {
    pub(crate) fn new(civilian_factories: u32, military_factories: u32) -> Self {
        let mut eco = Self {
            civilian_factories,
            military_factories,
            stockpile: EquipmentType::ALL.iter().map(|&t| (t, t.start_stock())).collect(),
            production_lines: default_lines(military_factories),
            daily_output: BTreeMap::new(),
            reinforce_cost: BTreeMap::new(),
            construction: ConstructionQueue::default(),
        };
        eco.recalc_daily_output();
        eco
    }

    /// Фабрики, занятые товарами народного потребления: 20% гражданских, минимум 1 (если фабрики вообще есть).
    pub fn consumer_goods_factories(&self) -> u32 {
        if self.civilian_factories == 0 {
            return 0;
        }
        (self.civilian_factories * CONSUMER_GOODS_SHARE_PCT / 100).max(1).min(self.civilian_factories)
    }

    /// Гражданские фабрики, свободные для стройки: всего минус ТНП.
    pub fn available_civilian_factories(&self) -> u32 {
        self.civilian_factories - self.consumer_goods_factories()
    }

    pub fn stock(&self, equipment: EquipmentType) -> u32 {
        self.stockpile.get(&equipment).copied().unwrap_or(0)
    }

    pub fn set_stock(&mut self, equipment: EquipmentType, amount: u32) {
        self.stockpile.insert(equipment, amount);
    }

    pub fn output_of(&self, equipment: EquipmentType) -> u32 {
        self.daily_output.get(&equipment).copied().unwrap_or(0)
    }

    pub fn cost_of(&self, equipment: EquipmentType) -> u32 {
        self.reinforce_cost.get(&equipment).copied().unwrap_or(0)
    }

    /// Заводы, фактически работающие на линиях: линии идут по порядку, и каждая получает не больше,
    /// чем осталось от `military_factories` (общий лимит не превышается никогда).
    pub fn effective_factories(&self) -> Vec<(EquipmentType, u32)> {
        let mut remaining = self.military_factories;
        self.production_lines
            .iter()
            .map(|line| {
                let working = line.assigned_factories.min(remaining);
                remaining -= working;
                (line.equipment, working)
            })
            .collect()
    }

    /// Свободные заводы: не назначенные ни на одну линию.
    pub fn free_factories(&self) -> u32 {
        let used: u32 = self.effective_factories().iter().map(|&(_, n)| n).sum();
        self.military_factories - used
    }

    /// Назначает линии `equipment` ровно `factories` заводов, но не больше, чем позволяет лимит
    /// (свободные заводы плюс уже стоявшие на этой линии). Возвращает фактически назначенное число.
    pub fn assign_factories(&mut self, equipment: EquipmentType, factories: u32) -> u32 {
        let on_line = self.effective_factories().iter().find(|(t, _)| *t == equipment).map_or(0, |&(_, n)| n);
        let assigned = factories.min(self.free_factories() + on_line);
        match self.production_lines.iter_mut().find(|l| l.equipment == equipment) {
            Some(line) => line.assigned_factories = assigned,
            None => self.production_lines.push(ProductionLine { equipment, assigned_factories: assigned }),
        }
        self.recalc_daily_output();
        assigned
    }

    /// Суточный выход = заводы линии * выход завода данного типа.
    pub(crate) fn recalc_daily_output(&mut self) {
        let mut output = BTreeMap::new();
        for (equipment, working) in self.effective_factories() {
            *output.entry(equipment).or_insert(0u32) += working * equipment.output_per_factory();
        }
        self.daily_output = output;
    }
}

/// Стартовые линии: доли `START_LINE_SHARES`, остаток от округления вниз — на пехотное снаряжение.
fn default_lines(military_factories: u32) -> Vec<ProductionLine> {
    let mut lines: Vec<ProductionLine> = START_LINE_SHARES
        .iter()
        .map(|&(equipment, pct)| ProductionLine { equipment, assigned_factories: military_factories * pct / 100 })
        .collect();
    let given: u32 = lines.iter().map(|l| l.assigned_factories).sum();
    lines[0].assigned_factories += military_factories - given;
    lines
}

/// Экономики всех стран по тегу. `BTreeMap` — детерминированный порядок обхода.
#[derive(Resource, Default)]
pub struct Economies(pub BTreeMap<String, CountryEconomy>);

/// Страна, за которую играет пользователь (её данные показывает верхняя панель).
#[derive(Resource)]
pub struct PlayerCountry(pub String);

/// Приказ: назначить производственной линии страны `factories` заводов.
/// Применяется в `apply_production_orders` с учётом лимита военных заводов.
#[derive(Event, Debug, Clone)]
pub struct SetProductionLine {
    pub country: String,
    pub equipment: EquipmentType,
    pub factories: u32,
}

/// Приказ: поставить в очередь стройки военный завод в стейте. Принимается, только если
/// страна контролирует стейт (`apply_construction_orders`).
#[derive(Event, Debug, Clone)]
pub struct QueueConstruction {
    pub country: String,
    pub state_id: u32,
}

/// Военные заводы по стейтам. `StateDef` в реестре разделяется через `Arc`, поэтому достроенные
/// заводы учитываются здесь: стартовые значения из файлов стейтов плюс построенные в игре.
#[derive(Resource, Default, Debug)]
pub struct StateFactories {
    pub military: BTreeMap<u32, u32>,
}

/// Стейтом управляет страна, если ей подчиняется больше половины его провинций.
pub fn state_controlled_by(state: &crate::map::StateDef, controllers: &ProvinceControllers, country: &str) -> bool {
    let held = state.provinces.iter().filter(|p| controllers.0.get(p).is_some_and(|c| c == country)).count();
    held * 2 > state.provinces.len()
}

/// Стартовый армейский опыт Германии и Франции.
const START_ARMY_XP: u32 = 10;
/// Потолок армейского опыта.
pub const MAX_ARMY_XP: u32 = 500;
/// Сколько суммарных дивизионо-часов боя на линии даёт 1 единицу опыта.
const COMBAT_HOURS_PER_XP: u32 = 24;

/// Армейский опыт по странам. Растёт от боевого времени дивизий на линии и не превышает MAX_ARMY_XP.
#[derive(Resource, Debug, Clone)]
pub struct ArmyExperience {
    /// Страна -> опыт (0..=500). BTreeMap — детерминированный порядок.
    pub xp_by_country: BTreeMap<String, u32>,
    /// Накопленные, ещё не превращённые в опыт дивизионо-часы боя (остаток меньше 24).
    combat_hours: BTreeMap<String, u32>,
}

impl Default for ArmyExperience {
    fn default() -> Self {
        let xp_by_country = ["GER", "FRA"].into_iter().map(|c| (c.to_string(), START_ARMY_XP)).collect();
        Self { xp_by_country, combat_hours: BTreeMap::new() }
    }
}

impl ArmyExperience {
    pub fn get(&self, country: &str) -> u32 {
        self.xp_by_country.get(country).copied().unwrap_or(0)
    }

    /// Начисляет опыт с потолком MAX_ARMY_XP.
    pub fn add(&mut self, country: &str, amount: u32) {
        let xp = self.xp_by_country.entry(country.to_string()).or_insert(0);
        *xp = xp.saturating_add(amount).min(MAX_ARMY_XP);
    }

    /// Один час боя: `fighting_divisions` дивизий страны стоят на линии. Каждые 24 накопленных
    /// дивизионо-часа дают +1 опыта; остаток переносится на следующие часы.
    pub fn record_combat_hour(&mut self, country: &str, fighting_divisions: u32) {
        let pending = self.combat_hours.entry(country.to_string()).or_insert(0);
        *pending += fighting_divisions;
        let gained = *pending / COMBAT_HOURS_PER_XP;
        *pending %= COMBAT_HOURS_PER_XP;
        if gained > 0 {
            self.add(country, gained);
        }
    }
}

/// Строка опыта для верхней панели.
pub fn army_xp_text(xp: u32) -> String {
    format!("XP Армии: {xp}")
}

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
            .add_event::<SetProductionLine>()
            .add_event::<QueueConstruction>()
            .init_resource::<StateFactories>()
            .init_resource::<ArmyExperience>()
            .init_resource::<SupplyNetwork>()
            .insert_resource(PlayerCountry(DEFAULT_PLAYER_COUNTRY.to_string()))
            .add_systems(Startup, (init_economies, init_supply_network, init_state_factories))
            // Суточный порядок: сеть снабжения -> статус снабжения дивизий (division::daily_supply);
            // производство на склад; затем пополнение со склада — только снабжённым дивизиям.
            .add_systems(
                Update,
                (
                    daily_production.after(ClockSet),
                    update_supply_network.after(ClockSet),
                    daily_reinforcement.after(daily_production).after(daily_supply),
                    economy_bar,
                    apply_production_orders,
                    production_panel,
                    apply_construction_orders,
                    // прогресс стройки — после производства того же дня
                    daily_construction.after(daily_production),
                    construction_panel,
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

/// Раз в сутки (00:00) заводы каждой линии производят свой тип снаряжения на склад страны:
/// `заводы линии * выход завода`. Заводов на линиях в сумме не больше `military_factories`.
fn daily_production(mut events: EventReader<DailyTickEvent>, mut economies: ResMut<Economies>) {
    for event in events.read() {
        for eco in economies.0.values_mut() {
            eco.recalc_daily_output();
            for (equipment, output) in eco.daily_output.clone() {
                let stock = eco.stock(equipment).saturating_add(output);
                eco.set_stock(equipment, stock);
            }
        }
        println!("[ЭКОНОМИКА] Сутки {}.{}.{}: производство учтено", event.day, event.month, event.year);
    }
}

/// Военные заводы по стейтам на старте — из файлов стейтов.
fn init_state_factories(registry: Res<StateRegistry>, mut factories: ResMut<StateFactories>) {
    factories.military = registry.states.iter().map(|s| (s.id, s.military_factories)).collect();
}

/// Применяет приказы `QueueConstruction`: в очередь попадает только стейт, которым управляет страна.
fn apply_construction_orders(
    mut orders: EventReader<QueueConstruction>,
    mut economies: ResMut<Economies>,
    registry: Res<StateRegistry>,
    controllers: Res<ProvinceControllers>,
) {
    for order in orders.read() {
        let Some(eco) = economies.0.get_mut(&order.country) else { continue };
        let Some(state) = registry.states.iter().find(|s| s.id == order.state_id) else {
            println!("[СТРОЙКА] Стейт #{} не найден", order.state_id);
            continue;
        };
        if !state_controlled_by(state, &controllers, &order.country) {
            println!("[СТРОЙКА] {} не контролирует стейт #{}", order.country, order.state_id);
            continue;
        }
        eco.construction.projects.push(ConstructionProject::military_factory(order.state_id));
        println!("[СТРОЙКА] {}: в очередь добавлен военный завод в стейте #{}", order.country, order.state_id);
    }
}

/// Раз в сутки (00:00) свободные гражданские фабрики (за вычетом ТНП) двигают очередь стройки.
/// Завершённый завод сразу прибавляется к стейту и к `military_factories` страны —
/// его можно назначать на производственные линии.
fn daily_construction(
    mut events: EventReader<DailyTickEvent>,
    mut economies: ResMut<Economies>,
    mut state_factories: ResMut<StateFactories>,
) {
    for _ in events.read() {
        for (tag, eco) in economies.0.iter_mut() {
            let available = eco.available_civilian_factories();
            for project in eco.construction.advance(available) {
                match project.building_type {
                    BuildingType::MilitaryFactory => {
                        *state_factories.military.entry(project.state_id).or_insert(0) += 1;
                        eco.military_factories += 1;
                        eco.recalc_daily_output();
                        println!("[СТРОЙКА] {tag}: достроен военный завод в стейте #{}", project.state_id);
                    }
                }
            }
        }
    }
}

/// Применяет приказы `SetProductionLine`: заводов на линию назначается не больше, чем свободно
/// (общее число заводов на линиях не превышает `military_factories`).
fn apply_production_orders(mut orders: EventReader<SetProductionLine>, mut economies: ResMut<Economies>) {
    for order in orders.read() {
        let Some(eco) = economies.0.get_mut(&order.country) else { continue };
        let assigned = eco.assign_factories(order.equipment, order.factories);
        if assigned < order.factories {
            println!(
                "[ЭКОНОМИКА] {}: на линию \"{}\" назначено {assigned} заводов из запрошенных {} (лимит {})",
                order.country,
                order.equipment.label(),
                order.factories,
                eco.military_factories
            );
        }
    }
}

/// Делит `total` очков по типам пропорционально весам: целая часть каждого, а остаток раздаётся
/// по одному типам с наибольшим дробным остатком (при равенстве — по порядку типов).
/// Детерминированно; сумма результата всегда равна `total`. Пустые веса дают пустой результат.
pub fn split_by_weights(total: u32, weights: &BTreeMap<EquipmentType, u32>) -> BTreeMap<EquipmentType, u32> {
    let weight_sum: u64 = weights.values().map(|&w| u64::from(w)).sum();
    if weight_sum == 0 {
        return BTreeMap::new();
    }
    let mut parts: Vec<(EquipmentType, u32, u64)> = weights
        .iter()
        .map(|(&t, &w)| {
            let scaled = u64::from(total) * u64::from(w);
            (t, (scaled / weight_sum) as u32, scaled % weight_sum)
        })
        .collect();
    let given: u32 = parts.iter().map(|p| p.1).sum();
    let mut left = total - given;
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by_key(|&i| (Reverse(parts[i].2), parts[i].0));
    for i in order {
        if left == 0 {
            break;
        }
        parts[i].1 += 1;
        left -= 1;
    }
    parts.into_iter().map(|(t, x, _)| (t, x)).collect()
}

/// Запрос дивизии на пополнение по типам: тип -> (очков прочности, единиц снаряжения).
/// Прочность делится по весам потребности (`needs`), цена очка у всех типов одинакова и равна
/// `сумма потребностей / максимальная прочность`, округление вверх.
pub fn division_request(
    strength_request: u32,
    needs: &BTreeMap<EquipmentType, u32>,
    max_strength: u32,
) -> BTreeMap<EquipmentType, (u32, u32)> {
    let weight_sum: u64 = needs.values().map(|&n| u64::from(n)).sum();
    split_by_weights(strength_request, needs)
        .into_iter()
        .filter(|&(_, points)| points > 0)
        .map(|(t, points)| {
            let cost = (u64::from(points) * weight_sum).div_ceil(u64::from(max_strength.max(1))) as u32;
            (t, (points, cost))
        })
        .collect()
}

/// Делит запас `available` между запросами `costs`: хватает на всех — каждому его запрос, иначе доля
/// `запрос * запас / сумма` с округлением вниз, а остаток раздаётся по одному в порядке следования.
pub fn share_stock(costs: &[u32], available: u32) -> Vec<u32> {
    let total: u64 = costs.iter().map(|&c| u64::from(c)).sum();
    if total <= u64::from(available) {
        return costs.to_vec();
    }
    let mut grants: Vec<u32> =
        costs.iter().map(|&c| (u64::from(c) * u64::from(available) / total) as u32).collect();
    let mut rest = u64::from(available) - grants.iter().map(|&g| u64::from(g)).sum::<u64>();
    for (grant, &cost) in grants.iter_mut().zip(costs) {
        if rest == 0 {
            break;
        }
        if *grant < cost {
            *grant += 1;
            rest -= 1;
        }
    }
    grants
}

/// Запрос одной дивизии в расчёте суток.
struct DivisionRequest {
    id: u32,
    entity: Entity,
    /// Нужды в снаряжении нет вообще (шаблон ничего не требует) — прочность восстанавливается даром.
    free_points: u32,
    per_type: BTreeMap<EquipmentType, (u32, u32)>,
}

/// Суточное пополнение. Снабжённые дивизии с неполной прочностью запрашивают до 50 очков в сутки.
/// Запрос раскладывается по типам снаряжения в долях потребности шаблона; каждый тип берётся с
/// собственного склада страны и ограничен им: если танков нет, а винтовки есть, пополняются только
/// пехотная и моторизованная части дивизии, а нехватка танков сдерживает рост общей прочности.
/// Отрезанные от снабжения дивизии не получают ничего. При нехватке типа его запас делится между
/// дивизиями пропорционально запросам (остаток — по порядку id). Всё детерминировано.
fn daily_reinforcement(
    mut events: EventReader<DailyTickEvent>,
    mut economies: ResMut<Economies>,
    mut divisions: Query<(Entity, &mut Division)>,
) {
    for _ in events.read() {
        for eco in economies.0.values_mut() {
            eco.reinforce_cost.clear();
        }

        let mut requests: BTreeMap<String, Vec<DivisionRequest>> = BTreeMap::new();
        let mut cut_off: BTreeMap<String, u32> = BTreeMap::new();
        for (entity, mut d) in divisions.iter_mut() {
            let deficit = d.max_strength.saturating_sub(d.current_strength);
            if deficit == 0 {
                d.equipment_shortage.clear();
                continue;
            }
            if !d.is_supplied {
                *cut_off.entry(d.owner.clone()).or_default() += 1; // отрезана: пополнение не отправляется
                continue;
            }
            let strength_request = deficit.min(REINFORCE_PER_DIVISION_PER_DAY);
            let per_type = division_request(strength_request, &d.equipment_needs, d.max_strength);
            let free_points = if per_type.is_empty() { strength_request } else { 0 };
            requests.entry(d.owner.clone()).or_default().push(DivisionRequest {
                id: d.id,
                entity,
                free_points,
                per_type,
            });
        }

        for (tag, skipped) in &cut_off {
            println!("[СНАБЖЕНИЕ] {tag}: отрезанных дивизий без пополнения: {skipped}");
        }

        for (tag, mut list) in requests {
            let Some(eco) = economies.0.get_mut(&tag) else { continue };
            list.sort_unstable_by_key(|r| r.id);

            // Выдача по каждому типу отдельно: grants[тип][i] — единицы, выданные i-й дивизии
            let mut grants: BTreeMap<EquipmentType, Vec<u32>> = BTreeMap::new();
            let mut short_types: BTreeSet<EquipmentType> = BTreeSet::new();
            for equipment in EquipmentType::ALL {
                let costs: Vec<u32> =
                    list.iter().map(|r| r.per_type.get(&equipment).map_or(0, |&(_, cost)| cost)).collect();
                if costs.iter().all(|&c| c == 0) {
                    continue;
                }
                let given = share_stock(&costs, eco.stock(equipment));
                if given.iter().zip(&costs).any(|(g, c)| g < c) {
                    short_types.insert(equipment);
                }
                grants.insert(equipment, given);
            }

            let mut spent: BTreeMap<EquipmentType, u32> = BTreeMap::new();
            for (i, request) in list.iter().enumerate() {
                let mut gain = request.free_points;
                let mut lacking = BTreeSet::new();
                for (&equipment, &(points, cost)) in &request.per_type {
                    let grant = grants.get(&equipment).map_or(0, |g| g[i]);
                    // Прирост по типу пропорционален выданной доле снаряжения
                    let part = if cost == 0 || grant >= cost {
                        points
                    } else {
                        (u64::from(grant) * u64::from(points) / u64::from(cost)) as u32
                    };
                    if grant < cost {
                        lacking.insert(equipment);
                    }
                    if part > 0 {
                        gain += part;
                        *spent.entry(equipment).or_default() += grant;
                    }
                }
                if let Ok((_, mut d)) = divisions.get_mut(request.entity) {
                    if gain > 0 {
                        d.current_strength = (d.current_strength + gain).min(d.max_strength);
                    }
                    d.equipment_shortage = lacking;
                }
            }

            for (&equipment, &amount) in &spent {
                let stock = eco.stock(equipment).saturating_sub(amount);
                eco.set_stock(equipment, stock);
                eco.reinforce_cost.insert(equipment, amount);
            }
            if !spent.is_empty() {
                let spent_text: Vec<String> =
                    spent.iter().map(|(t, n)| format!("{} {n}", t.label())).collect();
                let short_text: Vec<&str> = short_types.iter().map(|t| t.label()).collect();
                let note = if short_text.is_empty() { String::new() } else { format!(" (нехватка: {})", short_text.join(", ")) };
                println!("[СНАБЖЕНИЕ] {tag}: пополнено, потрачено: {}{note}", spent_text.join(", "));
            } else if !short_types.is_empty() {
                let short_text: Vec<&str> = short_types.iter().map(|t| t.label()).collect();
                println!("[СНАБЖЕНИЕ] {tag}: пополнение не выдано, нет на складе: {}", short_text.join(", "));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// Остатки ключевых типов для верхней панели: `Винтовки: 5000 (+120 / -50 день) | Танки: 600 | Пушки: 300`.
pub fn stockpile_text(eco: &CountryEconomy) -> String {
    let tanks = eco.stock(EquipmentType::LightTank) + eco.stock(EquipmentType::MediumTank);
    format!(
        "📦 Винтовки: {} (+{} / -{} день) | Танки: {} | Пушки: {}",
        eco.stock(EquipmentType::InfantryEquipment),
        eco.output_of(EquipmentType::InfantryEquipment),
        eco.cost_of(EquipmentType::InfantryEquipment),
        tanks,
        eco.stock(EquipmentType::Artillery)
    )
}

/// Гражданские фабрики для верхней панели: `Гражд. фабрики: 100 (Свободно: 80, ТНП: 20)`.
pub fn civilian_text(eco: &CountryEconomy) -> String {
    format!(
        "Гражд. фабрики: {} (Свободно: {}, ТНП: {})",
        eco.civilian_factories,
        eco.available_civilian_factories(),
        eco.consumer_goods_factories()
    )
}

/// Строка проекта очереди: `Военный завод в стейте #5 [30/7200] (6 фабр.)`.
pub fn project_text(project: &ConstructionProject, factories: u32) -> String {
    let name = match project.building_type {
        BuildingType::MilitaryFactory => "Военный завод",
    };
    format!("{name} в стейте #{} [{}/{}] ({factories} фабр.)", project.state_id, project.progress, project.cost)
}

/// Верхняя плашка: фабрики и склад страны игрока.
fn economy_bar(
    mut contexts: EguiContexts,
    economies: Res<Economies>,
    player: Res<PlayerCountry>,
    experience: Res<ArmyExperience>,
    colors: Res<CountryColors>,
) {
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    // Рамка и тег нации — в её настоящем цвете
    let country_color = colors.ui_color32(&player.0);
    let frame = egui::Frame::window(&ctx.style())
        .fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 200))
        .stroke(egui::Stroke::new(2.0_f32, country_color));

    egui::Window::new("economy_bar")
        .title_bar(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 10.0])
        .resizable(false)
        .frame(frame)
        .show(ctx, |ui| match economies.0.get(&player.0) {
            Some(eco) => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("[{}]", player.0)).color(country_color).strong());
                    ui.label(format!(
                        "🏭 {} | 🔨 Военные заводы: {} | {} | {}",
                        civilian_text(eco),
                        eco.military_factories,
                        stockpile_text(eco),
                        army_xp_text(experience.get(&player.0))
                    ));
                });
            }
            None => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("[{}]", player.0)).color(country_color).strong());
                    ui.label(format!("нет данных об экономике | {}", army_xp_text(experience.get(&player.0))));
                });
            }
        });
}

/// Окно производственных линий страны игрока: на каждом типе кнопки «−» и «+» меняют число заводов.
fn production_panel(
    mut contexts: EguiContexts,
    economies: Res<Economies>,
    player: Res<PlayerCountry>,
    mut orders: EventWriter<SetProductionLine>,
) {
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    let Some(eco) = economies.0.get(&player.0) else { return };
    let frame = egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 200));
    let working = eco.effective_factories();

    egui::Window::new("Производство")
        .anchor(egui::Align2::RIGHT_TOP, [-10.0, 52.0])
        .resizable(false)
        .collapsible(true)
        .frame(frame)
        .show(ctx, |ui| {
            ui.label(format!("Свободных заводов: {} из {}", eco.free_factories(), eco.military_factories));
            for equipment in EquipmentType::ALL {
                let factories = working.iter().find(|(t, _)| *t == equipment).map_or(0, |&(_, n)| n);
                ui.horizontal(|ui| {
                    if ui.add_enabled(factories > 0, egui::Button::new("−")).clicked() {
                        orders.send(SetProductionLine { country: player.0.clone(), equipment, factories: factories - 1 });
                    }
                    if ui.add_enabled(eco.free_factories() > 0, egui::Button::new("+")).clicked() {
                        orders.send(SetProductionLine { country: player.0.clone(), equipment, factories: factories + 1 });
                    }
                    ui.label(format!("{}: {} зав. (+{}/день)", equipment.label(), factories, eco.output_of(equipment)));
                });
            }
        });
}

/// Окно «Строительство»: кнопка постановки военного завода в выбранном на карте стейте и очередь проектов.
fn construction_panel(
    mut contexts: EguiContexts,
    economies: Res<Economies>,
    player: Res<PlayerCountry>,
    selected: Res<crate::map::SelectedProvince>,
    registry: Res<StateRegistry>,
    controllers: Res<ProvinceControllers>,
    state_factories: Res<StateFactories>,
    mut orders: EventWriter<QueueConstruction>,
) {
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    let Some(eco) = economies.0.get(&player.0) else { return };
    let frame = egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 200));
    // Стейт выбранной на карте провинции
    let state = selected.id.and_then(|province| registry.state_of(province));
    let allocation = eco.construction.allocation(eco.available_civilian_factories());

    egui::Window::new("Строительство")
        .anchor(egui::Align2::RIGHT_TOP, [-10.0, 260.0])
        .resizable(false)
        .collapsible(true)
        .frame(frame)
        .show(ctx, |ui| {
            match state {
                Some(state) => {
                    ui.label(format!(
                        "Стейт #{} (воен. заводов: {})",
                        state.id,
                        state_factories.military.get(&state.id).copied().unwrap_or(0)
                    ));
                }
                None => {
                    ui.label("Выберите провинцию своего стейта на карте");
                }
            }
            let can_build = state.is_some_and(|s| state_controlled_by(s, &controllers, &player.0));
            if ui.add_enabled(can_build, egui::Button::new("+ Построить военный завод в стейте")).clicked() {
                if let Some(state) = state {
                    orders.send(QueueConstruction { country: player.0.clone(), state_id: state.id });
                }
            }
            ui.separator();
            if eco.construction.projects.is_empty() {
                ui.label("Очередь пуста");
            }
            for (project, factories) in eco.construction.projects.iter().zip(&allocation) {
                ui.label(project_text(project, *factories));
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
        let (_, adjacency, _) = crate::map::build_id_texture_and_adjacency(&map);
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
    use crate::units::{DivisionTemplates, BASE_TEMPLATE, PLAIN_INFANTRY_TEMPLATE};

    use EquipmentType::*;

    fn spawn_division(app: &mut App, id: u32, province: u32, strength: u32, template: &str) -> Entity {
        let templates = DivisionTemplates::default();
        let mut d = Division::from_template(
            id,
            format!("Div {id}"),
            "GER".to_string(),
            province,
            template.to_string(),
            templates.get_or_base(template),
        );
        d.current_strength = strength;
        app.world_mut().spawn(d).id()
    }

    /// Пехота без поддержки: 1 винтовка на очко прочности — удобные круглые числа.
    fn plain(app: &mut App, id: u32, province: u32, strength: u32) -> Entity {
        spawn_division(app, id, province, strength, PLAIN_INFANTRY_TEMPLATE)
    }

    fn set_supplied(app: &mut App, e: Entity, supplied: bool) {
        app.world_mut().get_mut::<Division>(e).unwrap().is_supplied = supplied;
    }

    fn strength(app: &App, e: Entity) -> u32 {
        app.world().get::<Division>(e).unwrap().current_strength
    }

    fn eco(app: &App) -> &CountryEconomy {
        &app.world().resource::<Economies>().0["GER"]
    }

    fn stock(app: &App, equipment: EquipmentType) -> u32 {
        eco(app).stock(equipment)
    }

    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    /// Приложение с экономикой GER: заданный запас пехотного снаряжения, остальные типы — нули.
    fn app_with_stock(infantry: u32) -> App {
        app_with_stocks(&[(InfantryEquipment, infantry)])
    }

    fn app_with_stocks(stocks: &[(EquipmentType, u32)]) -> App {
        let mut app = App::new();
        let mut economy = CountryEconomy::new(10, 10);
        for t in EquipmentType::ALL {
            economy.set_stock(t, 0);
        }
        for &(t, n) in stocks {
            economy.set_stock(t, n);
        }
        app.add_event::<DailyTickEvent>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), economy)])))
            .add_systems(Update, daily_reinforcement);
        app
    }

    #[test]
    fn supplied_division_is_reinforced_from_the_stockpile() {
        let mut app = app_with_stock(1000);
        let d = plain(&mut app, 1, 1, 800);
        day(&mut app);
        // до 50 единиц прочности в сутки, по винтовке на единицу
        assert_eq!(strength(&app, d), 850);
        assert_eq!((stock(&app, InfantryEquipment), eco(&app).cost_of(InfantryEquipment)), (950, 50));
        // второй день: остаток дефицита 50 -> полностью укомплектована
        day(&mut app);
        assert_eq!(strength(&app, d), 900);
        assert_eq!((stock(&app, InfantryEquipment), eco(&app).cost_of(InfantryEquipment)), (900, 50));
        // полный состав больше ничего не просит
        day(&mut app);
        assert_eq!((stock(&app, InfantryEquipment), eco(&app).cost_of(InfantryEquipment)), (900, 0));
    }

    #[test]
    fn cut_off_division_gets_nothing_and_costs_nothing() {
        let mut app = app_with_stock(1000);
        let d = plain(&mut app, 1, 1, 700);
        set_supplied(&mut app, d, false);
        let before = app.world().get::<Division>(d).unwrap().losses;
        day(&mut app);
        assert_eq!(strength(&app, d), 700, "отрезанной дивизии подкрепления не отправляются");
        assert_eq!((stock(&app, InfantryEquipment), eco(&app).cost_of(InfantryEquipment)), (1000, 0));
        assert_eq!(app.world().get::<Division>(d).unwrap().losses, before);

        // снабжение восстановилось — пополнение возобновляется
        set_supplied(&mut app, d, true);
        day(&mut app);
        assert_eq!(strength(&app, d), 750);
        assert_eq!((stock(&app, InfantryEquipment), eco(&app).cost_of(InfantryEquipment)), (950, 50));
    }

    #[test]
    fn cut_off_divisions_do_not_take_a_share_of_a_short_stockpile() {
        // Запас 60 при запросах 2 x 50 от снабжённых: поровну по 30; отрезанная третья не получает ничего
        let mut app = app_with_stock(60);
        let a = plain(&mut app, 1, 1, 800);
        let b = plain(&mut app, 2, 1, 800);
        let cut = plain(&mut app, 3, 1, 800);
        set_supplied(&mut app, cut, false);
        day(&mut app);
        assert_eq!((strength(&app, a), strength(&app, b), strength(&app, cut)), (830, 830, 800));
        assert_eq!((stock(&app, InfantryEquipment), eco(&app).cost_of(InfantryEquipment)), (0, 60));
    }

    #[test]
    fn standard_division_draws_every_equipment_type_it_needs() {
        // потребность стандартной дивизии: 900 пехотного, 10 оснащения, 24 орудия; запрос 50 очков
        // раскладывается на 48 / 1 / 1, а цена очка 934 / 900 округляется вверх: 50 / 2 / 2
        let mut app = app_with_stocks(&[(InfantryEquipment, 1000), (SupportEquipment, 100), (Artillery, 100)]);
        let d = spawn_division(&mut app, 1, 1, 800, BASE_TEMPLATE);
        day(&mut app);
        assert_eq!(strength(&app, d), 850);
        assert_eq!(stock(&app, InfantryEquipment), 950);
        assert_eq!(stock(&app, SupportEquipment), 98);
        assert_eq!(stock(&app, Artillery), 98);
        assert!(app.world().get::<Division>(d).unwrap().equipment_shortage.is_empty());
    }

    #[test]
    fn missing_tanks_hold_back_a_tank_division_but_infantry_part_is_refilled() {
        // танковая дивизия: потребность 200 лёгких танков и 200 пехотного (моторизованные), прочность 300/600
        let mut app = app_with_stocks(&[(InfantryEquipment, 1000), (LightTank, 0)]);
        let d = spawn_division(&mut app, 1, 1, 300, "Panzer-Division");
        day(&mut app);
        // запрос 50 очков делится 25 / 25; цена очка 400 / 600, то есть по 17 на тип.
        // Танков нет: растёт только пехотная часть (+25), а не полные +50
        assert_eq!(strength(&app, d), 325);
        assert_eq!(stock(&app, InfantryEquipment), 1000 - 17);
        assert_eq!(stock(&app, LightTank), 0);
        assert_eq!(eco(&app).cost_of(LightTank), 0, "танки не тратятся, их нет");
        assert_eq!(app.world().get::<Division>(d).unwrap().equipment_shortage, BTreeSet::from([LightTank]));

        // танки поступили на склад — следующий день пополняет обе части
        app.world_mut().resource_mut::<Economies>().0.get_mut("GER").unwrap().set_stock(LightTank, 100);
        day(&mut app);
        assert_eq!(strength(&app, d), 375, "+25 пехотной части и +25 танковой");
        assert_eq!(stock(&app, LightTank), 100 - 17);
        assert!(app.world().get::<Division>(d).unwrap().equipment_shortage.is_empty());
    }

    #[test]
    fn partial_tank_stock_gives_a_proportional_share() {
        // танков 8 из нужных 17: танковая часть получает 8 * 25 / 17 = 11 очков, пехотная — полные 25
        let mut app = app_with_stocks(&[(InfantryEquipment, 1000), (LightTank, 8)]);
        let d = spawn_division(&mut app, 1, 1, 300, "Panzer-Division");
        day(&mut app);
        assert_eq!(strength(&app, d), 300 + 25 + 11);
        assert_eq!(stock(&app, LightTank), 0);
        assert_eq!(eco(&app).cost_of(LightTank), 8);
    }

    #[test]
    fn empty_warehouse_reinforces_nothing_and_reports_every_shortage() {
        let mut app = app_with_stocks(&[]);
        let d = spawn_division(&mut app, 1, 1, 300, "Panzer-Division");
        day(&mut app);
        assert_eq!(strength(&app, d), 300);
        assert_eq!(
            app.world().get::<Division>(d).unwrap().equipment_shortage,
            BTreeSet::from([InfantryEquipment, LightTank])
        );
    }

    #[test]
    fn full_daily_chain_attrition_then_supply_check_then_reinforcement() {
        // Хаб в провинции 1 снабжает 1 и 2; провинция 9 далеко от любого хаба
        let mut app = App::new();
        let neighbors = BTreeMap::from([(1, BTreeSet::from([2])), (2, BTreeSet::from([1]))]);
        let mut economy = CountryEconomy::new(10, 10);
        economy.set_stock(InfantryEquipment, 1000);
        app.add_event::<DailyTickEvent>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), economy)])))
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

        let connected = plain(&mut app, 1, 2, 800);
        let isolated = plain(&mut app, 2, 9, 800);

        day(&mut app);
        // подключённая: снабжена, +50; изолированная в тот же день отмечена как отрезанная,
        // теряет 2% (16) и подкреплений не получает
        assert_eq!(strength(&app, connected), 850);
        assert_eq!(strength(&app, isolated), 784);
        assert!(!app.world().get::<Division>(isolated).unwrap().is_supplied);
        assert_eq!((stock(&app, InfantryEquipment), eco(&app).cost_of(InfantryEquipment)), (950, 50));
    }
}

#[cfg(test)]
mod equipment_tests {
    use super::*;

    use EquipmentType::*;

    fn needs(pairs: &[(EquipmentType, u32)]) -> BTreeMap<EquipmentType, u32> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn split_by_weights_always_sums_to_total_and_is_deterministic() {
        // 50 очков по весам 900 / 10 / 24: целые 48 / 0 / 1, остаток 1 уходит типу с наибольшим дробным остатком
        let split = split_by_weights(50, &needs(&[(InfantryEquipment, 900), (SupportEquipment, 10), (Artillery, 24)]));
        assert_eq!(split, needs(&[(InfantryEquipment, 48), (SupportEquipment, 1), (Artillery, 1)]));
        // ровно пополам, нечётный остаток — по порядку типов при равных остатках
        let tie = split_by_weights(51, &needs(&[(InfantryEquipment, 200), (LightTank, 200)]));
        assert_eq!(tie, needs(&[(InfantryEquipment, 26), (LightTank, 25)]));
        for total in 0..200 {
            let parts = split_by_weights(total, &needs(&[(InfantryEquipment, 7), (Artillery, 3), (LightTank, 11)]));
            assert_eq!(parts.values().sum::<u32>(), total);
        }
        assert!(split_by_weights(50, &BTreeMap::new()).is_empty());
        assert_eq!(split_by_weights(0, &needs(&[(Artillery, 5)])), needs(&[(Artillery, 0)]));
    }

    #[test]
    fn division_request_prices_each_type_by_the_same_ratio() {
        // стандартная дивизия: W = 934, прочность 900 -> 50 очков = 48 / 1 / 1, цена 50 / 2 / 2
        let request = division_request(
            50,
            &needs(&[(InfantryEquipment, 900), (SupportEquipment, 10), (Artillery, 24)]),
            900,
        );
        assert_eq!(
            request,
            BTreeMap::from([(InfantryEquipment, (48, 50)), (SupportEquipment, (1, 2)), (Artillery, (1, 2))])
        );
        // без потребности запроса нет
        assert!(division_request(50, &BTreeMap::new(), 900).is_empty());
        // нулевая максимальная прочность не вызывает деления на ноль
        assert_eq!(division_request(10, &needs(&[(Artillery, 5)]), 0), BTreeMap::from([(Artillery, (10, 50))]));
    }

    #[test]
    fn share_stock_splits_proportionally_with_remainder_in_order() {
        assert_eq!(share_stock(&[50, 50], 1000), vec![50, 50], "хватает всем");
        assert_eq!(share_stock(&[50, 50, 50], 60), vec![20, 20, 20]);
        // 100 / 3 -> по 33, остаток 1 первому из тех, у кого есть место
        assert_eq!(share_stock(&[50, 50, 50], 100), vec![34, 33, 33]);
        assert_eq!(share_stock(&[50, 50], 0), vec![0, 0]);
        assert_eq!(share_stock(&[], 10), Vec::<u32>::new());
        assert_eq!(share_stock(&[0, 30], 10), vec![0, 10], "нулевой запрос ничего не получает");
    }

    #[test]
    fn start_stock_and_labels() {
        let eco = CountryEconomy::new(5, 10);
        assert_eq!(eco.stock(InfantryEquipment), 5000);
        assert_eq!((eco.stock(SupportEquipment), eco.stock(Artillery), eco.stock(LightTank), eco.stock(MediumTank)), (500, 300, 600, 0));
        assert_eq!(InfantryEquipment.label(), "Винтовки");
        assert_eq!(Artillery.label(), "Пушки");
        // выход одного завода: 5 винтовок, 1 орудие или 1 танк
        assert_eq!(
            (InfantryEquipment.output_per_factory(), Artillery.output_per_factory(), LightTank.output_per_factory(), MediumTank.output_per_factory()),
            (5, 1, 1, 1)
        );
    }

    #[test]
    fn start_lines_use_all_military_factories_and_never_exceed_them() {
        for mil in [0, 1, 3, 4, 10, 37, 100] {
            let eco = CountryEconomy::new(0, mil);
            let assigned: u32 = eco.production_lines.iter().map(|l| l.assigned_factories).sum();
            assert_eq!(assigned, mil, "{mil} заводов распределены без остатка");
            assert_eq!(eco.free_factories(), 0);
        }
        // 10 заводов: 15% -> 1 пушки, 25% -> 2 лёгких танка, 10% -> 1 оснащения, остаток до 10 — винтовки (6)
        let eco = CountryEconomy::new(0, 10);
        let lines: BTreeMap<_, _> = eco.production_lines.iter().map(|l| (l.equipment, l.assigned_factories)).collect();
        assert_eq!(lines[&InfantryEquipment], 6);
        assert_eq!((lines[&SupportEquipment], lines[&Artillery], lines[&LightTank], lines[&MediumTank]), (1, 1, 2, 0));
        assert_eq!((eco.output_of(InfantryEquipment), eco.output_of(LightTank), eco.output_of(Artillery)), (30, 2, 1));
    }

    #[test]
    fn assign_factories_respects_the_military_factory_limit() {
        let mut eco = CountryEconomy::new(0, 10);
        // заводы все заняты: на средние танки свободных нет
        assert_eq!(eco.assign_factories(MediumTank, 3), 0);
        // освобождаем 4 завода на винтовках — теперь 3 уходят в средние танки, ещё 1 свободен
        assert_eq!(eco.assign_factories(InfantryEquipment, 2), 2);
        assert_eq!(eco.free_factories(), 4);
        assert_eq!(eco.assign_factories(MediumTank, 3), 3);
        assert_eq!(eco.free_factories(), 1);
        assert_eq!(eco.output_of(MediumTank), 3);
        // запрос больше лимита обрезается до свободного (1 свободный + 3 уже на линии)
        assert_eq!(eco.assign_factories(MediumTank, 100), 4);
        assert_eq!(eco.free_factories(), 0);
        // общая сумма заводов на линиях не больше лимита
        let total: u32 = eco.production_lines.iter().map(|l| l.assigned_factories).sum();
        assert!(total <= eco.military_factories);
    }

    #[test]
    fn oversubscribed_lines_are_clamped_when_producing() {
        let mut eco = CountryEconomy::new(0, 10);
        eco.production_lines = vec![
            ProductionLine { equipment: InfantryEquipment, assigned_factories: 8 },
            ProductionLine { equipment: LightTank, assigned_factories: 8 },
        ];
        eco.recalc_daily_output();
        // винтовки заняли 8 из 10; на танки остаётся 2
        assert_eq!(eco.effective_factories(), vec![(InfantryEquipment, 8), (LightTank, 2)]);
        assert_eq!((eco.output_of(InfantryEquipment), eco.output_of(LightTank)), (40, 2));
        assert_eq!(eco.free_factories(), 0);
    }

    fn production_app() -> App {
        let mut app = App::new();
        let mut eco = CountryEconomy::new(0, 10);
        for t in EquipmentType::ALL {
            eco.set_stock(t, 0);
        }
        app.add_event::<DailyTickEvent>()
            .add_event::<SetProductionLine>()
            .add_event::<QueueConstruction>()
            .init_resource::<StateFactories>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), eco)])))
            .add_systems(Update, (apply_production_orders, daily_production.after(apply_production_orders)));
        app
    }

    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    #[test]
    fn daily_production_fills_each_stockpile_from_its_line() {
        let mut app = production_app();
        day(&mut app);
        let eco = &app.world().resource::<Economies>().0["GER"];
        // линии 10 заводов: винтовки 6 * 5, оснащение 1 * 3, пушки 1 * 1, лёгкие танки 2 * 1, средние 0
        assert_eq!(eco.stock(InfantryEquipment), 30);
        assert_eq!(eco.stock(SupportEquipment), 3);
        assert_eq!(eco.stock(Artillery), 1);
        assert_eq!(eco.stock(LightTank), 2);
        assert_eq!(eco.stock(MediumTank), 0);
        day(&mut app);
        assert_eq!(app.world().resource::<Economies>().0["GER"].stock(InfantryEquipment), 60, "выход накапливается");
    }

    #[test]
    fn production_orders_move_factories_between_lines_within_the_limit() {
        let mut app = production_app();
        // переводим 4 завода с винтовок на средние танки; сверх свободных — не больше лимита
        app.world_mut().send_event(SetProductionLine { country: "GER".into(), equipment: InfantryEquipment, factories: 2 });
        app.update();
        app.world_mut().send_event(SetProductionLine { country: "GER".into(), equipment: MediumTank, factories: 50 });
        app.update();
        let eco = &app.world().resource::<Economies>().0["GER"];
        assert_eq!(eco.free_factories(), 0);
        assert_eq!(eco.output_of(MediumTank), 4, "6 -> 2 освободило 4 завода, остальное обрезано лимитом");
        let total: u32 = eco.production_lines.iter().map(|l| l.assigned_factories).sum();
        assert_eq!(total, 10);
        // неизвестная страна приказ игнорирует без паники
        app.world_mut().send_event(SetProductionLine { country: "XXX".into(), equipment: Artillery, factories: 1 });
        app.update();
        day(&mut app);
        assert_eq!(app.world().resource::<Economies>().0["GER"].stock(MediumTank), 4);
    }

    #[test]
    fn top_panel_shows_rifles_tanks_and_guns() {
        let mut eco = CountryEconomy::new(0, 10);
        eco.set_stock(InfantryEquipment, 1234);
        eco.set_stock(LightTank, 40);
        eco.set_stock(MediumTank, 2);
        eco.set_stock(Artillery, 77);
        eco.reinforce_cost.insert(InfantryEquipment, 50);
        assert_eq!(stockpile_text(&eco), "📦 Винтовки: 1234 (+30 / -50 день) | Танки: 42 | Пушки: 77");
    }
}

#[cfg(test)]
mod experience_tests {
    use super::*;

    #[test]
    fn army_experience_starts_at_ten_for_germany_and_france() {
        let xp = ArmyExperience::default();
        assert_eq!((xp.get("GER"), xp.get("FRA")), (10, 10));
        assert_eq!(xp.get("ITA"), 0, "прочие страны начинают с нуля");
    }

    #[test]
    fn experience_is_capped_at_five_hundred() {
        let mut xp = ArmyExperience::default();
        xp.add("GER", 489);
        assert_eq!(xp.get("GER"), 499);
        xp.add("GER", 1);
        assert_eq!(xp.get("GER"), MAX_ARMY_XP);
        xp.add("GER", 1_000_000);
        assert_eq!(xp.get("GER"), 500, "потолок 500");
        xp.add("ITA", 3);
        assert_eq!(xp.get("ITA"), 3, "новая страна появляется при первом начислении");
    }

    #[test]
    fn every_twenty_four_division_hours_give_one_xp_with_carry() {
        let mut xp = ArmyExperience::default();
        // 23 дивизионо-часа — опыта ещё нет
        for _ in 0..23 {
            xp.record_combat_hour("GER", 1);
        }
        assert_eq!(xp.get("GER"), 10);
        xp.record_combat_hour("GER", 1);
        assert_eq!(xp.get("GER"), 11);
        // несколько дивизий на линии копят быстрее: 100 дивизионо-часов за раз = +4, остаток 4
        xp.record_combat_hour("FRA", 100);
        assert_eq!(xp.get("FRA"), 14);
        xp.record_combat_hour("FRA", 20); // остаток 4 + 20 = 24 -> ещё +1
        assert_eq!(xp.get("FRA"), 15);
        // страны копят независимо
        assert_eq!(xp.get("GER"), 11);
        // на потолке счётчик не переполняется и опыт не растёт
        xp.add("FRA", 1_000);
        xp.record_combat_hour("FRA", 480);
        assert_eq!(xp.get("FRA"), 500);
    }

    #[test]
    fn army_xp_text_matches_the_panel_format() {
        assert_eq!(army_xp_text(10), "XP Армии: 10");
        assert_eq!(army_xp_text(500), "XP Армии: 500");
    }
}

#[cfg(test)]
mod construction_tests {
    use super::*;
    use std::sync::Arc;

    use crate::map::StateDef;

    fn project(state_id: u32, progress: u32) -> ConstructionProject {
        ConstructionProject { progress, ..ConstructionProject::military_factory(state_id) }
    }

    fn queue(projects: Vec<ConstructionProject>) -> ConstructionQueue {
        ConstructionQueue { projects }
    }

    #[test]
    fn consumer_goods_take_twenty_percent_but_at_least_one() {
        let factories = |total: u32| {
            let eco = CountryEconomy::new(total, 0);
            (eco.consumer_goods_factories(), eco.available_civilian_factories())
        };
        assert_eq!(factories(0), (0, 0), "без фабрик нечем занимать");
        assert_eq!(factories(1), (1, 0), "минимум одна фабрика на ТНП");
        assert_eq!(factories(4), (1, 3), "20% от 4 — это 0, но минимум 1");
        assert_eq!(factories(5), (1, 4));
        assert_eq!(factories(10), (2, 8));
        assert_eq!(factories(100), (20, 80));
        assert_eq!(factories(804), (160, 644), "целочисленное округление вниз");
        for total in 0..300 {
            let (cg, free) = factories(total);
            assert_eq!(cg + free, total, "ТНП и свободные вместе дают все гражданские фабрики");
        }
    }

    #[test]
    fn a_project_takes_at_most_fifteen_factories_and_the_rest_flow_on() {
        let q = queue(vec![project(1, 0), project(2, 0), project(3, 0)]);
        assert_eq!(q.allocation(6), vec![6, 0, 0]);
        assert_eq!(q.allocation(15), vec![15, 0, 0]);
        assert_eq!(q.allocation(20), vec![15, 5, 0], "остаток переходит на следующий проект");
        assert_eq!(q.allocation(40), vec![15, 15, 10]);
        assert_eq!(q.allocation(100), vec![15, 15, 15], "лишние фабрики простаивают");
        assert_eq!(q.allocation(0), vec![0, 0, 0]);
        assert!(queue(vec![]).allocation(10).is_empty());
    }

    #[test]
    fn nearly_finished_project_does_not_waste_factories() {
        // до завершения 10 очков — хватит двух фабрик, остальные уходят дальше
        let q = queue(vec![project(1, 7190), project(2, 0), project(3, 0)]);
        assert_eq!(q.allocation(20), vec![2, 15, 3]);
    }

    #[test]
    fn each_factory_adds_five_points_per_day() {
        let mut q = queue(vec![project(1, 0)]);
        assert!(q.advance(6).is_empty());
        assert_eq!(q.projects[0].progress, 30);
        let mut q = queue(vec![project(1, 0), project(2, 0)]);
        q.advance(20); // 15 фабрик на первый (75 очков), 5 на второй (25 очков)
        assert_eq!((q.projects[0].progress, q.projects[1].progress), (75, 25));
        // без свободных фабрик прогресса нет
        q.advance(0);
        assert_eq!((q.projects[0].progress, q.projects[1].progress), (75, 25));
    }

    #[test]
    fn fifteen_factories_finish_a_military_factory_in_ninety_six_days() {
        let mut q = queue(vec![ConstructionProject::military_factory(7)]);
        for day in 1..=95 {
            assert!(q.advance(15).is_empty(), "на день {day} проект ещё не готов");
        }
        assert_eq!(q.projects[0].progress, 7125);
        let done = q.advance(15);
        assert_eq!(done.len(), 1, "96 суток * 75 очков = 7200");
        assert_eq!((done[0].state_id, done[0].progress, done[0].cost), (7, 7200, 7200));
        assert!(q.projects.is_empty(), "готовый проект уходит из очереди");
    }

    #[test]
    fn leftover_factories_continue_with_the_next_project_on_completion_day() {
        let mut q = queue(vec![project(1, 7190), project(2, 0), project(3, 0)]);
        let done = q.advance(20);
        assert_eq!(done.iter().map(|p| p.state_id).collect::<Vec<_>>(), vec![1]);
        assert_eq!(q.projects.iter().map(|p| (p.state_id, p.progress)).collect::<Vec<_>>(), vec![(2, 75), (3, 15)]);
        // прогресс не превышает стоимость
        let mut q = queue(vec![project(1, 7199)]);
        assert_eq!(q.advance(15)[0].progress, 7200);
    }

    fn state(id: u32, provinces: Vec<u32>, military: u32) -> Arc<StateDef> {
        Arc::new(StateDef {
            id,
            name: format!("S{id}"),
            owner: "GER".into(),
            provinces,
            victory_points: vec![],
            civilian_factories: 0,
            military_factories: military,
        })
    }

    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    fn daily_app(economy: CountryEconomy, state_factories: &[(u32, u32)]) -> App {
        let mut app = App::new();
        app.add_event::<DailyTickEvent>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), economy)])))
            .insert_resource(StateFactories { military: state_factories.iter().copied().collect() })
            .add_systems(Update, daily_construction);
        app
    }

    #[test]
    fn completed_factory_joins_the_state_and_the_nation_and_can_be_assigned() {
        // 10 гражданских: 2 на ТНП, 8 свободных; проекту не хватает 5 очков — нужна одна фабрика
        let mut economy = CountryEconomy::new(10, 10);
        economy.construction.projects.push(project(5, 7195));
        let mut app = daily_app(economy, &[(5, 2)]);
        day(&mut app);

        let eco = &app.world().resource::<Economies>().0["GER"];
        assert!(eco.construction.projects.is_empty());
        assert_eq!(eco.military_factories, 11, "военных заводов нации стало на 1 больше");
        assert_eq!(app.world().resource::<StateFactories>().military[&5], 3, "и в стейте тоже");
        assert_eq!(eco.free_factories(), 1, "новый завод свободен для линий");

        // сразу назначаем его на производство средних танков
        let mut economies = app.world_mut().resource_mut::<Economies>();
        let eco = economies.0.get_mut("GER").unwrap();
        assert_eq!(eco.assign_factories(EquipmentType::MediumTank, 1), 1);
        assert_eq!(eco.output_of(EquipmentType::MediumTank), 1);
    }

    #[test]
    fn several_projects_can_finish_on_the_same_day() {
        let mut economy = CountryEconomy::new(10, 10);
        economy.construction.projects.extend([project(1, 7195), project(2, 7195), project(1, 0)]);
        let mut app = daily_app(economy, &[(1, 0)]);
        day(&mut app);
        let eco = &app.world().resource::<Economies>().0["GER"];
        assert_eq!(eco.military_factories, 12);
        assert_eq!(eco.construction.projects.len(), 1, "третий проект ещё строится");
        // 8 фабрик: по одной на первые два, 6 оставшихся — на третий (30 очков)
        assert_eq!(eco.construction.projects[0].progress, 30);
        let state_factories = &app.world().resource::<StateFactories>().military;
        assert_eq!((state_factories[&1], state_factories[&2]), (1, 1), "для нового стейта запись создаётся");
    }

    #[test]
    fn construction_is_paid_only_by_non_consumer_factories() {
        // 1 гражданская фабрика целиком уходит на ТНП: стройка стоит
        let mut economy = CountryEconomy::new(1, 10);
        economy.construction.projects.push(project(5, 0));
        let mut app = daily_app(economy, &[]);
        day(&mut app);
        assert_eq!(app.world().resource::<Economies>().0["GER"].construction.projects[0].progress, 0);
    }

    fn order_app() -> App {
        let mut app = App::new();
        app.add_event::<QueueConstruction>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), CountryEconomy::new(10, 10))])))
            .insert_resource(StateRegistry {
                states: vec![state(7, vec![1, 2, 3], 0), state(8, vec![4, 5], 0)],
                ..Default::default()
            })
            .insert_resource(ProvinceControllers(BTreeMap::from([
                (1, "GER".to_string()),
                (2, "GER".to_string()),
                (3, "FRA".to_string()),
                (4, "GER".to_string()),
                (5, "FRA".to_string()),
            ])))
            .add_systems(Update, apply_construction_orders);
        app
    }

    fn order(app: &mut App, country: &str, state_id: u32) {
        app.world_mut().send_event(QueueConstruction { country: country.into(), state_id });
        app.update();
    }

    #[test]
    fn only_controlled_states_can_be_queued() {
        let mut app = order_app();
        // стейт 7: 2 из 3 провинций у GER — управляется
        order(&mut app, "GER", 7);
        // стейт 8: 1 из 2 — поровну, это не контроль
        order(&mut app, "GER", 8);
        // несуществующий стейт и неизвестная страна приказ не создают
        order(&mut app, "GER", 99);
        order(&mut app, "XXX", 7);
        let queue = &app.world().resource::<Economies>().0["GER"].construction;
        assert_eq!(queue.projects, vec![ConstructionProject::military_factory(7)]);
        assert_eq!(queue.projects[0].cost, 7200);
        // тот же стейт можно поставить ещё раз: проекты независимы
        order(&mut app, "GER", 7);
        assert_eq!(app.world().resource::<Economies>().0["GER"].construction.projects.len(), 2);
    }

    #[test]
    fn state_control_needs_a_strict_majority() {
        let controllers = ProvinceControllers(BTreeMap::from([
            (1, "GER".to_string()),
            (2, "GER".to_string()),
            (3, "FRA".to_string()),
            (4, "FRA".to_string()),
        ]));
        assert!(state_controlled_by(&state(1, vec![1, 2, 3], 0), &controllers, "GER"));
        assert!(!state_controlled_by(&state(1, vec![1, 2, 3, 4], 0), &controllers, "GER"), "ровно половина — не контроль");
        assert!(!state_controlled_by(&state(1, vec![], 0), &controllers, "GER"), "пустой стейт никому не принадлежит");
        assert!(!state_controlled_by(&state(1, vec![9], 0), &controllers, "GER"), "провинция без контроллера");
    }

    #[test]
    fn state_factories_start_from_state_files() {
        let mut app = App::new();
        app.init_resource::<StateFactories>()
            .insert_resource(StateRegistry {
                states: vec![state(1, vec![1], 4), state(2, vec![2], 0)],
                ..Default::default()
            })
            .add_systems(Update, init_state_factories);
        app.update();
        assert_eq!(app.world().resource::<StateFactories>().military, BTreeMap::from([(1, 4), (2, 0)]));
    }

    #[test]
    fn panel_texts_match_the_requested_format() {
        let eco = CountryEconomy::new(100, 10);
        assert_eq!(civilian_text(&eco), "Гражд. фабрики: 100 (Свободно: 80, ТНП: 20)");
        assert_eq!(
            project_text(&project(5, 30), 6),
            "Военный завод в стейте #5 [30/7200] (6 фабр.)"
        );
        assert_eq!(
            project_text(&ConstructionProject::military_factory(12), 15),
            "Военный завод в стейте #12 [0/7200] (15 фабр.)"
        );
    }
}
