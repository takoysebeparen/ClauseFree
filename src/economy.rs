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

/// Стартовая эффективность линии, %.
pub const START_EFFICIENCY: u32 = 20;
/// Эффективность после смены типа снаряжения на линии, %.
pub const SWITCH_EFFICIENCY: u32 = 10;
/// Потолок эффективности новой линии, %.
pub const DEFAULT_MAX_EFFICIENCY: u32 = 50;
/// Прирост эффективности линии в сутки, %.
const EFFICIENCY_GAIN_PER_DAY: u32 = 1;
/// Стоимость конверсии фабрики (гражданская <-> военная) в очках стройки.
pub const CONVERSION_COST: u32 = 4000;

/// Стартовый торговый контракт GER: продавец и число вложенных фабрик (3 * 8 = 24 стали).
const START_TRADE_SELLER: &str = "SWE";
const START_TRADE_FACTORIES: u32 = 3;

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

/// Потолок штрафа к выпуску из-за нехватки сырья, %: линия не падает ниже 10% потенциала.
pub const MAX_RESOURCE_PENALTY: u32 = 90;
/// Сколько единиц ресурса в сутки покупает одна гражданская фабрика, вложенная в импорт.
pub const RESOURCE_PER_FACTORY: u32 = 8;
/// Природная добыча в тестовых экономиках: домашний пул при законе 50% — 1 000 000 каждого ресурса.
#[cfg(test)]
const TEST_NATURAL_RESOURCE: u32 = 2_000_000;

// ---------------------------------------------------------------------------
// Стратегические ресурсы
// ---------------------------------------------------------------------------

/// Стратегическое сырьё. Пул страны — суточный приток: он не накапливается и не расходуется,
/// а сравнивается с суточной потребностью заводов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResourceType {
    Steel,
    Tungsten,
    Aluminium,
    Rubber,
    Oil,
}

impl ResourceType {
    /// Все ресурсы в фиксированном порядке (детерминированный обход).
    pub const ALL: [ResourceType; 5] =
        [ResourceType::Steel, ResourceType::Tungsten, ResourceType::Aluminium, ResourceType::Rubber, ResourceType::Oil];

    pub fn label(self) -> &'static str {
        match self {
            ResourceType::Steel => "Сталь",
            ResourceType::Tungsten => "Вольфрам",
            ResourceType::Aluminium => "Алюминий",
            ResourceType::Rubber => "Резина",
            ResourceType::Oil => "Нефть",
        }
    }

    /// Ключ ресурса в файлах стейтов (`steel`, `tungsten`, ...); хром и прочее игнорируются.
    pub fn from_key(key: &str) -> Option<ResourceType> {
        match key {
            "steel" => Some(ResourceType::Steel),
            "tungsten" => Some(ResourceType::Tungsten),
            "aluminium" => Some(ResourceType::Aluminium),
            "rubber" => Some(ResourceType::Rubber),
            "oil" => Some(ResourceType::Oil),
            _ => None,
        }
    }

    /// Родительный падеж для строки дефицита: «Дефицит Стали».
    fn genitive(self) -> &'static str {
        match self {
            ResourceType::Steel => "Стали",
            ResourceType::Tungsten => "Вольфрама",
            ResourceType::Aluminium => "Алюминия",
            ResourceType::Rubber => "Резины",
            ResourceType::Oil => "Нефти",
        }
    }
}

/// Контракт импорта: покупатель вкладывает гражданские фабрики и получает ресурс у продавца.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeOrder {
    pub resource: ResourceType,
    /// Тег страны-экспортёра.
    pub seller: String,
    /// Число вложенных гражданских фабрик.
    pub factories: u32,
}

impl TradeOrder {
    /// Сколько единиц ресурса в сутки даёт контракт.
    pub fn amount(&self) -> u32 {
        self.factories * RESOURCE_PER_FACTORY
    }
}

/// Торговый закон страны: какая доля добычи уходит на мировой рынок.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportLaw {
    /// Исторический стандарт 1936 года: 50% добычи в экспортный пул, 50% остаётся дома.
    #[default]
    ExportFocus,
}

impl ExportLaw {
    pub fn export_share_pct(self) -> u32 {
        match self {
            ExportLaw::ExportFocus => 50,
        }
    }
}

/// Домашний пул GER для тестов (значения прежней заглушки).
#[cfg(test)]
pub fn ger_start_resources() -> BTreeMap<ResourceType, u32> {
    BTreeMap::from([
        (ResourceType::Steel, 60),
        (ResourceType::Aluminium, 20),
        (ResourceType::Tungsten, 15),
        (ResourceType::Rubber, 5),
        (ResourceType::Oil, 10),
    ])
}

/// Процент дефицита ресурса: 0 при `available >= required`, иначе `(required - available) * 100 / required`
/// (целочисленно, округление вниз).
pub fn deficit_percent(available: u32, required: u32) -> u32 {
    if required == 0 || available >= required {
        return 0;
    }
    (u64::from(required - available) * 100 / u64::from(required)) as u32
}

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

    /// Расход сырья одним работающим заводом в сутки.
    pub fn resource_cost(self) -> &'static [(ResourceType, u32)] {
        use ResourceType::{Aluminium, Oil, Steel, Tungsten};
        match self {
            EquipmentType::InfantryEquipment => &[(Steel, 1)],
            EquipmentType::SupportEquipment => &[(Steel, 1), (Aluminium, 1)],
            EquipmentType::Artillery => &[(Steel, 2), (Tungsten, 1)],
            EquipmentType::LightTank => &[(Steel, 2), (Tungsten, 1), (Oil, 1)],
            EquipmentType::MediumTank => &[(Steel, 3), (Tungsten, 2), (Oil, 1)],
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
    /// Текущая эффективность, % (10..=100): выпуск умножается на неё. Растёт на 1% в сутки до потолка.
    pub efficiency: u32,
    /// Потолок эффективности, % (базово 50).
    pub max_efficiency: u32,
}

impl ProductionLine {
    /// Новая линия с начальной эффективностью 20% и потолком 50%.
    pub fn new(equipment: EquipmentType, assigned_factories: u32) -> Self {
        Self { equipment, assigned_factories, efficiency: START_EFFICIENCY, max_efficiency: DEFAULT_MAX_EFFICIENCY }
    }

    /// Суточный выпуск: `заводы * выход_завода * эффективность / 100`, но не меньше 1 при назначенных заводах.
    pub fn output(&self, working_factories: u32) -> u32 {
        if working_factories == 0 {
            return 0;
        }
        (working_factories * self.equipment.output_per_factory() * self.efficiency / 100).max(1)
    }

    /// Выпуск с учётом дефицита сырья: `output * (100 - penalty) / 100`, минимум 1 при назначенных заводах.
    /// Штраф ограничен `MAX_RESOURCE_PENALTY`.
    pub fn output_with_penalty(&self, working_factories: u32, penalty: u32) -> u32 {
        let base = self.output(working_factories);
        if base == 0 {
            return 0;
        }
        let penalty = penalty.min(MAX_RESOURCE_PENALTY);
        (base * (100 - penalty) / 100).max(1)
    }

    /// Суточный прирост эффективности: +1% до потолка.
    fn grow(&mut self) {
        self.efficiency = (self.efficiency + EFFICIENCY_GAIN_PER_DAY).min(self.max_efficiency.max(self.efficiency));
    }
}

/// Тип здания, которое можно строить.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildingType {
    MilitaryFactory,
    /// Конверсия гражданской фабрики в военный завод.
    CivToMilConversion,
    /// Конверсия военного завода в гражданскую фабрику.
    MilToCivConversion,
}

impl BuildingType {
    /// Стоимость проекта в очках стройки.
    pub fn cost(self) -> u32 {
        match self {
            BuildingType::MilitaryFactory => MILITARY_FACTORY_COST,
            BuildingType::CivToMilConversion | BuildingType::MilToCivConversion => CONVERSION_COST,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BuildingType::MilitaryFactory => "Военный завод",
            BuildingType::CivToMilConversion => "Конверсия Civ -> Mil",
            BuildingType::MilToCivConversion => "Конверсия Mil -> Civ",
        }
    }
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
    /// Проект военного завода (в игре создаётся через `new`; удобный конструктор для тестов).
    #[cfg(test)]
    pub fn military_factory(state_id: u32) -> Self {
        Self::new(BuildingType::MilitaryFactory, state_id)
    }

    /// Проект нужного типа со стоимостью по таблице.
    pub fn new(building_type: BuildingType, state_id: u32) -> Self {
        Self { state_id, building_type, progress: 0, cost: building_type.cost() }
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
    /// Природная добыча из стейтов страны (суточная; нет записи — ноль).
    pub natural_resources: BTreeMap<ResourceType, u32>,
    /// Торговый закон: доля добычи, уходящая в экспортный пул.
    pub export_law: ExportLaw,
    /// Активные контракты импорта этой страны (она — покупатель).
    pub trade_orders: Vec<TradeOrder>,
    /// Фабрики, временно переданные этой стране покупателями (она — продавец). Пересчитывается
    /// из контрактов других стран (`recompute_trade_links`).
    pub imported_factories: u32,
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
            natural_resources: BTreeMap::new(),
            export_law: ExportLaw::default(),
            trade_orders: Vec::new(),
            imported_factories: 0,
        };
        eco.recalc_daily_output();
        eco
    }

    /// Добыча ресурса в стейтах страны.
    pub fn natural(&self, resource: ResourceType) -> u32 {
        self.natural_resources.get(&resource).copied().unwrap_or(0)
    }

    /// Экспортный пул: `добыча * доля закона / 100`.
    pub fn exportable_amount(&self, resource: ResourceType) -> u32 {
        (u64::from(self.natural(resource)) * u64::from(self.export_law.export_share_pct()) / 100) as u32
    }

    /// Остаётся дома: добыча минус экспортный пул.
    pub fn domestic_available(&self, resource: ResourceType) -> u32 {
        self.natural(resource) - self.exportable_amount(resource)
    }

    /// Собственный базовый пул для производства (без импорта).
    pub fn resource(&self, resource: ResourceType) -> u32 {
        self.domestic_available(resource)
    }

    /// Заменяет добычу (смена владельцев стейтов) и пересчитывает выпуск.
    pub fn set_natural_resources(&mut self, natural: BTreeMap<ResourceType, u32>) {
        self.natural_resources = natural;
        self.recalc_daily_output();
    }

    /// Тест: экономика с практически неограниченной добычей, чтобы штрафы не мешали проверкам других систем.
    #[cfg(test)]
    pub(crate) fn new_abundant(civilian_factories: u32, military_factories: u32) -> Self {
        let mut eco = Self::new(civilian_factories, military_factories);
        eco.set_natural_resources(ResourceType::ALL.iter().map(|&r| (r, TEST_NATURAL_RESOURCE)).collect());
        eco
    }

    /// Тест: задаёт домашний пул так, чтобы `domestic_available` равнялся `pool`.
    #[cfg(test)]
    pub(crate) fn set_pool(&mut self, pool: BTreeMap<ResourceType, u32>) {
        let keep = 100 - self.export_law.export_share_pct();
        self.set_natural_resources(pool.into_iter().map(|(r, v)| (r, (u64::from(v) * 100).div_ceil(u64::from(keep)) as u32)).collect());
    }

    /// Суточная потребность в сырье: сумма `работающие заводы * норма` по всем линиям.
    pub fn resource_demand(&self) -> BTreeMap<ResourceType, u32> {
        let mut demand = BTreeMap::new();
        for (equipment, working) in self.effective_factories() {
            for &(resource, per_factory) in equipment.resource_cost() {
                *demand.entry(resource).or_insert(0u32) += working * per_factory;
            }
        }
        demand
    }

    /// Сырьё, не занятое заводами: пул минус потребность (не ниже нуля).
    pub fn resource_free(&self, resource: ResourceType) -> u32 {
        let required = self.resource_demand().get(&resource).copied().unwrap_or(0);
        self.total_resource(resource).saturating_sub(required)
    }

    /// Дефицит каждого востребованного ресурса, % (только ресурсы с потребностью > 0).
    pub fn resource_deficits(&self) -> BTreeMap<ResourceType, u32> {
        self.resource_demand()
            .into_iter()
            .map(|(resource, required)| (resource, deficit_percent(self.total_resource(resource), required)))
            .collect()
    }

    /// Самый острый дефицит среди ресурсов типа снаряжения: (ресурс, %). При равенстве — первый по порядку `ResourceType`.
    fn worst_shortage(&self, equipment: EquipmentType, deficits: &BTreeMap<ResourceType, u32>) -> Option<(ResourceType, u32)> {
        let mut worst: Option<(ResourceType, u32)> = None;
        for &(resource, _) in equipment.resource_cost() {
            let percent = deficits.get(&resource).copied().unwrap_or(0);
            if percent > worst.map_or(0, |(_, p)| p) {
                worst = Some((resource, percent));
            }
        }
        worst.map(|(resource, percent)| (resource, percent.min(MAX_RESOURCE_PENALTY)))
    }

    /// Штраф линии к выпуску, %: максимальный дефицит её ресурсов, не выше `MAX_RESOURCE_PENALTY`.
    #[cfg(test)]
    pub fn resource_penalty(&self, equipment: EquipmentType) -> u32 {
        self.worst_shortage(equipment, &self.resource_deficits()).map_or(0, |(_, p)| p)
    }

    /// Ресурс с наибольшим дефицитом для линии типа `equipment` и штраф от него (`None`, если сырья хватает).
    pub fn resource_shortage(&self, equipment: EquipmentType) -> Option<(ResourceType, u32)> {
        self.worst_shortage(equipment, &self.resource_deficits())
    }

    /// Фабрики, занятые товарами народного потребления: 20% гражданских, минимум 1 (если фабрики вообще есть).
    pub fn consumer_goods_factories(&self) -> u32 {
        if self.civilian_factories == 0 {
            return 0;
        }
        (self.civilian_factories * CONSUMER_GOODS_SHARE_PCT / 100).max(1).min(self.civilian_factories)
    }

    /// Фабрики, вложенные в импорт, по всем активным заказам.
    pub fn dedicated_trade_factories(&self) -> u32 {
        self.trade_orders.iter().map(|o| o.factories).sum()
    }

    /// Собственные гражданские фабрики, свободные от ТНП и торговли (без переданных продавцу).
    pub fn own_available_civilian_factories(&self) -> u32 {
        self.civilian_factories
            .saturating_sub(self.consumer_goods_factories())
            .saturating_sub(self.dedicated_trade_factories())
    }

    /// Гражданские фабрики, свободные для стройки: всего минус ТНП минус торговля, плюс фабрики,
    /// переданные покупателями этой стране как продавцу.
    pub fn available_civilian_factories(&self) -> u32 {
        self.own_available_civilian_factories() + self.imported_factories
    }

    /// Ресурс, получаемый по импорту: сумма `фабрики * RESOURCE_PER_FACTORY` по заказам этого ресурса.
    pub fn imported_resources(&self, resource: ResourceType) -> u32 {
        self.trade_orders.iter().filter(|o| o.resource == resource).map(TradeOrder::amount).sum()
    }

    /// Эффективный приток: базовый пул плюс импорт.
    pub fn total_resource(&self, resource: ResourceType) -> u32 {
        self.resource(resource).saturating_add(self.imported_resources(resource))
    }

    /// Открывает контракт импорта. Нельзя: ноль фабрик или больше, чем свободно собственных
    /// гражданских фабрик (ТНП и уже вложенные в торговлю не трогаются).
    pub fn create_trade_order(&mut self, resource: ResourceType, seller: &str, factories: u32) -> Result<(), &'static str> {
        if factories == 0 {
            return Err("нужна хотя бы одна фабрика");
        }
        if factories > self.own_available_civilian_factories() {
            return Err("недостаточно свободных гражданских фабрик");
        }
        self.trade_orders.push(TradeOrder { resource, seller: seller.to_string(), factories });
        self.recalc_daily_output();
        Ok(())
    }

    /// Отменяет контракт по индексу; фабрики возвращаются в свободные.
    pub fn cancel_trade_order(&mut self, index: usize) -> Result<TradeOrder, &'static str> {
        if index >= self.trade_orders.len() {
            return Err("такого заказа нет");
        }
        let order = self.trade_orders.remove(index);
        self.recalc_daily_output();
        Ok(order)
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
            None => self.production_lines.push(ProductionLine::new(equipment, assigned)),
        }
        self.recalc_daily_output();
        assigned
    }

    /// Суточный выход линий: `заводы * выход завода * эффективность / 100` (минимум 1 при назначенных заводах).
    pub(crate) fn recalc_daily_output(&mut self) {
        let deficits = self.resource_deficits();
        let mut output = BTreeMap::new();
        for (line, (equipment, working)) in self.production_lines.iter().zip(self.effective_factories()) {
            let penalty = self.worst_shortage(equipment, &deficits).map_or(0, |(_, p)| p);
            *output.entry(equipment).or_insert(0u32) += line.output_with_penalty(working, penalty);
        }
        self.daily_output = output;
    }

    /// Суточный прирост эффективности всех линий; затем выпуск пересчитывается под новые значения.
    fn grow_efficiency(&mut self) {
        for line in &mut self.production_lines {
            line.grow();
        }
        self.recalc_daily_output();
    }

    /// Меняет тип снаряжения линии вместе с её заводами. Эффективность падает до `SWITCH_EFFICIENCY` (10%).
    /// Нельзя: линии `from` нет, тип не меняется или линия `to` уже работает (есть заводы).
    /// Пустая линия типа `to` заменяется переведённой.
    pub fn switch_line_equipment(&mut self, from: EquipmentType, to: EquipmentType) -> Result<(), &'static str> {
        if from == to {
            return Err("тип не меняется");
        }
        if !self.production_lines.iter().any(|l| l.equipment == from) {
            return Err("такой линии нет");
        }
        if self.production_lines.iter().any(|l| l.equipment == to && l.assigned_factories > 0) {
            return Err("линия этого типа уже работает");
        }
        self.production_lines.retain(|l| !(l.equipment == to && l.assigned_factories == 0));
        let line = self.production_lines.iter_mut().find(|l| l.equipment == from).expect("линия from проверена выше");
        line.equipment = to;
        line.efficiency = SWITCH_EFFICIENCY;
        self.recalc_daily_output();
        Ok(())
    }

    /// Убирает один военный завод (конверсия в гражданскую фабрику). Если свободных нет, завод снимается
    /// с последней линии, где он работает, чтобы назначения оставались в пределах лимита.
    fn release_military_factory(&mut self) {
        if self.free_factories() == 0 {
            if let Some(line) = self.production_lines.iter_mut().rev().find(|l| l.assigned_factories > 0) {
                line.assigned_factories -= 1;
            }
        }
        self.military_factories = self.military_factories.saturating_sub(1);
        self.recalc_daily_output();
    }
}

/// Стартовые линии: доли `START_LINE_SHARES`, остаток от округления вниз — на пехотное снаряжение.
fn default_lines(military_factories: u32) -> Vec<ProductionLine> {
    let mut lines: Vec<ProductionLine> = START_LINE_SHARES
        .iter()
        .map(|&(equipment, pct)| ProductionLine::new(equipment, military_factories * pct / 100))
        .collect();
    let given: u32 = lines.iter().map(|l| l.assigned_factories).sum();
    lines[0].assigned_factories += military_factories - given;
    lines
}

/// Пересчитывает фабрики, переданные продавцам: у каждой страны `imported_factories` — сумма фабрик
/// из заказов других стран, где она указана продавцом. Заказы к неизвестным странам фабрик не передают.
pub fn recompute_trade_links(economies: &mut BTreeMap<String, CountryEconomy>) {
    let mut received: BTreeMap<String, u32> = BTreeMap::new();
    for (buyer, eco) in economies.iter() {
        for order in &eco.trade_orders {
            if order.seller != *buyer {
                *received.entry(order.seller.clone()).or_insert(0) += order.factories;
            }
        }
    }
    for (tag, eco) in economies.iter_mut() {
        eco.imported_factories = received.get(tag).copied().unwrap_or(0);
    }
}

/// Сколько единиц ресурса у `seller` уже покупают другие страны по активным заказам.
pub fn exported_from(economies: &BTreeMap<String, CountryEconomy>, seller: &str, resource: ResourceType) -> u32 {
    economies
        .iter()
        .filter(|(buyer, _)| buyer.as_str() != seller)
        .flat_map(|(_, eco)| eco.trade_orders.iter())
        .filter(|o| o.seller == seller && o.resource == resource)
        .map(TradeOrder::amount)
        .sum()
}

/// Экспортный лимит продавца: (свободно, всего) единиц ресурса. Неизвестная страна — (0, 0).
pub fn free_export(economies: &BTreeMap<String, CountryEconomy>, seller: &str, resource: ResourceType) -> (u32, u32) {
    let total = economies.get(seller).map_or(0, |e| e.exportable_amount(resource));
    (total.saturating_sub(exported_from(economies, seller, resource)), total)
}

/// Открывает контракт покупателя с проверкой экспортной квоты продавца:
/// `уже_экспортируется + factories * 8 <= exportable_amount`. Затем обновляет связи фабрик.
pub fn open_trade_order(
    economies: &mut BTreeMap<String, CountryEconomy>,
    buyer: &str,
    resource: ResourceType,
    seller: &str,
    factories: u32,
) -> Result<(), &'static str> {
    if seller == buyer || !economies.contains_key(seller) {
        return Err("неверный продавец");
    }
    if !economies.contains_key(buyer) {
        return Err("неизвестный покупатель");
    }
    let (free, _) = free_export(economies, seller, resource);
    if u64::from(factories) * u64::from(RESOURCE_PER_FACTORY) > u64::from(free) {
        return Err("превышен экспортный лимит продавца");
    }
    match economies.get_mut(buyer) {
        Some(eco) => eco.create_trade_order(resource, seller, factories)?,
        None => return Err("неизвестный покупатель"),
    }
    recompute_trade_links(economies);
    Ok(())
}

/// Если суммарный экспорт продавца превысил новый лимит (потеря стейтов), заказы пропорционально
/// уменьшаются: `factories * допустимо / всего` (целые, вниз); нулевые заказы удаляются.
/// Возвращает `true`, если что-то изменилось.
pub fn enforce_export_quotas(economies: &mut BTreeMap<String, CountryEconomy>) -> bool {
    let pairs: BTreeSet<(String, ResourceType)> = economies
        .values()
        .flat_map(|e| e.trade_orders.iter().map(|o| (o.seller.clone(), o.resource)))
        .collect();
    let mut changed = false;
    for (seller, resource) in pairs {
        let limit = economies.get(&seller).map_or(0, |e| e.exportable_amount(resource));
        let allowed = u64::from(limit / RESOURCE_PER_FACTORY);
        let total: u64 = economies
            .iter()
            .filter(|(buyer, _)| **buyer != seller)
            .flat_map(|(_, e)| e.trade_orders.iter())
            .filter(|o| o.seller == seller && o.resource == resource)
            .map(|o| u64::from(o.factories))
            .sum();
        if total <= allowed {
            continue;
        }
        changed = true;
        for (buyer, eco) in economies.iter_mut() {
            if *buyer == seller {
                continue;
            }
            for order in eco.trade_orders.iter_mut().filter(|o| o.seller == seller && o.resource == resource) {
                order.factories = (u64::from(order.factories) * allowed / total) as u32;
            }
            eco.trade_orders.retain(|o| o.factories > 0);
        }
    }
    if changed {
        for eco in economies.values_mut() {
            eco.recalc_daily_output();
        }
        recompute_trade_links(economies);
    }
    changed
}

/// Природные ресурсы по странам с учётом контроля: стейт достаётся стране, контролирующей больше
/// половины его провинций, иначе — владельцу.
pub fn natural_by_controller(
    registry: &StateRegistry,
    controllers: &ProvinceControllers,
) -> BTreeMap<String, BTreeMap<ResourceType, u32>> {
    let mut totals: BTreeMap<String, BTreeMap<ResourceType, u32>> = BTreeMap::new();
    for state in &registry.states {
        let mut held: BTreeMap<&str, usize> = BTreeMap::new();
        for province in &state.provinces {
            if let Some(controller) = controllers.0.get(province) {
                *held.entry(controller.as_str()).or_insert(0) += 1;
            }
        }
        let holder = held
            .iter()
            .find(|(_, n)| **n * 2 > state.provinces.len())
            .map_or(state.owner.as_str(), |(&tag, _)| tag);
        let entry = totals.entry(holder.to_string()).or_default();
        for (&resource, &amount) in &state.resources {
            let slot = entry.entry(resource).or_insert(0);
            *slot = slot.saturating_add(amount);
        }
    }
    totals
}

/// Приказ игрока: открыть контракт импорта.
#[derive(Event, Debug, Clone)]
pub struct CreateTradeOrder {
    pub country: String,
    pub resource: ResourceType,
    pub seller: String,
    pub factories: u32,
}

/// Приказ игрока: отменить контракт импорта по индексу.
#[derive(Event, Debug, Clone)]
pub struct CancelTradeOrder {
    pub country: String,
    pub order_index: usize,
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

/// Приказ: сменить тип снаряжения линии (эффективность линии падает до 10%).
#[derive(Event, Debug, Clone)]
pub struct SwitchProductionLine {
    pub country: String,
    pub from: EquipmentType,
    pub to: EquipmentType,
}

/// Приказ: поставить в очередь стройки проект в стейте — военный завод или конверсию фабрики.
/// Принимается, только если страна контролирует стейт и есть что строить / конвертировать
/// (`apply_construction_orders`).
#[derive(Event, Debug, Clone)]
pub struct QueueConstruction {
    pub country: String,
    pub state_id: u32,
    pub building_type: BuildingType,
}

/// Военные заводы по стейтам. `StateDef` в реестре разделяется через `Arc`, поэтому достроенные
/// заводы учитываются здесь: стартовые значения из файлов стейтов плюс построенные в игре.
#[derive(Resource, Default, Debug)]
pub struct StateFactories {
    pub military: BTreeMap<u32, u32>,
    /// Гражданские фабрики по стейтам (нужны для конверсий).
    pub civilian: BTreeMap<u32, u32>,
}

impl StateFactories {
    pub fn military_in(&self, state_id: u32) -> u32 {
        self.military.get(&state_id).copied().unwrap_or(0)
    }

    pub fn civilian_in(&self, state_id: u32) -> u32 {
        self.civilian.get(&state_id).copied().unwrap_or(0)
    }
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
            .add_event::<SwitchProductionLine>()
            .add_event::<CreateTradeOrder>()
            .add_event::<CancelTradeOrder>()
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
                    daily_natural_resources.after(ClockSet),
                    daily_production.after(ClockSet).after(daily_natural_resources),
                    update_supply_network.after(ClockSet),
                    daily_reinforcement.after(daily_production).after(daily_supply),
                    economy_bar,
                    apply_production_orders,
                    apply_switch_orders,
                    apply_trade_orders,
                    trade_panel,
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
    let mut natural = registry.resources_by_owner();
    for (tag, (civ, mil)) in registry.factories_by_owner() {
        total_civ += u64::from(civ);
        total_mil += u64::from(mil);
        let mut eco = CountryEconomy::new(civ, mil);
        eco.set_natural_resources(natural.remove(&tag).unwrap_or_default());
        economies.0.insert(tag, eco);
    }
    // стартовый контракт GER: 24 стали у Швеции за 3 гражданские фабрики (проходит экспортную квоту SWE)
    if let Err(reason) = open_trade_order(
        &mut economies.0,
        DEFAULT_PLAYER_COUNTRY,
        ResourceType::Steel,
        START_TRADE_SELLER,
        START_TRADE_FACTORIES,
    ) {
        println!("[ТОРГОВЛЯ] стартовый контракт GER/SWE не открыт: {reason}");
    }
    println!(
        "[ЭКОНОМИКА] Стран: {}, гражданских фабрик: {total_civ}, военных заводов: {total_mil}",
        economies.0.len()
    );
}

/// Раз в сутки (00:00) заводы каждой линии производят свой тип снаряжения на склад страны:
/// `заводы линии * выход завода * эффективность / 100` (минимум 1 при назначенных заводах).
/// Заводов на линиях в сумме не больше `military_factories`. После выпуска эффективность
/// каждой линии растёт на 1% до её потолка.
fn daily_production(mut events: EventReader<DailyTickEvent>, mut economies: ResMut<Economies>) {
    for event in events.read() {
        // потерянные ресурсы продавцов урезают заказы; фабрики продавцам обновляются до выпуска и стройки
        enforce_export_quotas(&mut economies.0);
        recompute_trade_links(&mut economies.0);
        for eco in economies.0.values_mut() {
            eco.recalc_daily_output();
            for (equipment, output) in eco.daily_output.clone() {
                let stock = eco.stock(equipment).saturating_add(output);
                eco.set_stock(equipment, stock);
            }
            eco.grow_efficiency();
        }
        println!("[ЭКОНОМИКА] Сутки {}.{}.{}: производство учтено", event.day, event.month, event.year);
    }
}

/// Раз в сутки пересчитывает природную добычу стран по контролю над стейтами (до производства).
fn daily_natural_resources(
    mut events: EventReader<DailyTickEvent>,
    registry: Res<StateRegistry>,
    controllers: Res<ProvinceControllers>,
    mut economies: ResMut<Economies>,
) {
    if events.read().count() == 0 {
        return;
    }
    let mut totals = natural_by_controller(&registry, &controllers);
    for (tag, eco) in economies.0.iter_mut() {
        eco.natural_resources = totals.remove(tag).unwrap_or_default();
    }
}

/// Военные заводы по стейтам на старте — из файлов стейтов.
fn init_state_factories(registry: Res<StateRegistry>, mut factories: ResMut<StateFactories>) {
    factories.military = registry.states.iter().map(|s| (s.id, s.military_factories)).collect();
    factories.civilian = registry.states.iter().map(|s| (s.id, s.civilian_factories)).collect();
}

/// Можно ли поставить проект в очередь: страна управляет стейтом, а для конверсии в стейте и в стране
/// остаются фабрики нужного вида (с учётом уже стоящих в очереди конверсий того же типа).
pub fn can_queue(
    kind: BuildingType,
    state: &crate::map::StateDef,
    controllers: &ProvinceControllers,
    eco: &CountryEconomy,
    state_factories: &StateFactories,
    country: &str,
) -> Result<(), &'static str> {
    if !state_controlled_by(state, controllers, country) {
        return Err("страна не контролирует стейт");
    }
    let queued = |same_state: bool| {
        eco.construction
            .projects
            .iter()
            .filter(|p| p.building_type == kind && (!same_state || p.state_id == state.id))
            .count() as u32
    };
    match kind {
        BuildingType::MilitaryFactory => Ok(()),
        BuildingType::CivToMilConversion => {
            if state_factories.civilian_in(state.id) <= queued(true) {
                Err("в стейте нет гражданских фабрик для конверсии")
            } else if eco.civilian_factories <= queued(false) {
                Err("у страны нет гражданских фабрик для конверсии")
            } else {
                Ok(())
            }
        }
        BuildingType::MilToCivConversion => {
            if state_factories.military_in(state.id) <= queued(true) {
                Err("в стейте нет военных заводов для конверсии")
            } else if eco.military_factories <= queued(false) {
                Err("у страны нет военных заводов для конверсии")
            } else {
                Ok(())
            }
        }
    }
}

/// Применяет приказы `QueueConstruction` через `can_queue`.
fn apply_construction_orders(
    mut orders: EventReader<QueueConstruction>,
    mut economies: ResMut<Economies>,
    registry: Res<StateRegistry>,
    controllers: Res<ProvinceControllers>,
    state_factories: Res<StateFactories>,
) {
    for order in orders.read() {
        let Some(eco) = economies.0.get_mut(&order.country) else { continue };
        let Some(state) = registry.states.iter().find(|s| s.id == order.state_id) else {
            println!("[СТРОЙКА] Стейт #{} не найден", order.state_id);
            continue;
        };
        if let Err(reason) = can_queue(order.building_type, state, &controllers, eco, &state_factories, &order.country) {
            println!("[СТРОЙКА] {}: {} в стейте #{} отклонено: {reason}", order.country, order.building_type.label(), order.state_id);
            continue;
        }
        eco.construction.projects.push(ConstructionProject::new(order.building_type, order.state_id));
        println!("[СТРОЙКА] {}: в очередь добавлено: {} в стейте #{}", order.country, order.building_type.label(), order.state_id);
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
                    BuildingType::CivToMilConversion => {
                        // к моменту завершения фабрика могла исчезнуть (например, при захвате): тогда проект пропадает
                        if state_factories.civilian_in(project.state_id) == 0 || eco.civilian_factories == 0 {
                            println!("[СТРОЙКА] {tag}: конверсия Civ -> Mil в стейте #{} отменена — нет гражданской фабрики", project.state_id);
                            continue;
                        }
                        *state_factories.civilian.entry(project.state_id).or_insert(0) -= 1;
                        *state_factories.military.entry(project.state_id).or_insert(0) += 1;
                        eco.civilian_factories -= 1;
                        eco.military_factories += 1;
                        eco.recalc_daily_output();
                        println!("[СТРОЙКА] {tag}: гражданская фабрика в стейте #{} стала военным заводом", project.state_id);
                    }
                    BuildingType::MilToCivConversion => {
                        if state_factories.military_in(project.state_id) == 0 || eco.military_factories == 0 {
                            println!("[СТРОЙКА] {tag}: конверсия Mil -> Civ в стейте #{} отменена — нет военного завода", project.state_id);
                            continue;
                        }
                        *state_factories.military.entry(project.state_id).or_insert(0) -= 1;
                        *state_factories.civilian.entry(project.state_id).or_insert(0) += 1;
                        eco.release_military_factory();
                        eco.civilian_factories += 1;
                        println!("[СТРОЙКА] {tag}: военный завод в стейте #{} стал гражданской фабрикой", project.state_id);
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

/// Применяет приказы `SwitchProductionLine`: смена типа снаряжения сбрасывает эффективность линии до 10%.
fn apply_switch_orders(mut orders: EventReader<SwitchProductionLine>, mut economies: ResMut<Economies>) {
    for order in orders.read() {
        let Some(eco) = economies.0.get_mut(&order.country) else { continue };
        match eco.switch_line_equipment(order.from, order.to) {
            Ok(()) => println!(
                "[ЭКОНОМИКА] {}: линия \"{}\" переведена на \"{}\", эффективность {SWITCH_EFFICIENCY}%",
                order.country,
                order.from.label(),
                order.to.label()
            ),
            Err(reason) => println!("[ЭКОНОМИКА] {}: смена типа линии отклонена: {reason}", order.country),
        }
    }
}

/// Приказы торговли: открытие и отмена контрактов. Продавец должен существовать и отличаться от покупателя.
fn apply_trade_orders(
    mut creates: EventReader<CreateTradeOrder>,
    mut cancels: EventReader<CancelTradeOrder>,
    mut economies: ResMut<Economies>,
) {
    for order in creates.read() {
        match open_trade_order(&mut economies.0, &order.country, order.resource, &order.seller, order.factories) {
            Ok(()) => println!(
                "[ТОРГОВЛЯ] {}: +{} {} от {} за {} фабр.",
                order.country,
                order.factories * RESOURCE_PER_FACTORY,
                order.resource.label(),
                order.seller,
                order.factories
            ),
            Err(reason) => println!("[ТОРГОВЛЯ] {}: заказ отклонён: {reason}", order.country),
        }
    }
    for order in cancels.read() {
        let Some(eco) = economies.0.get_mut(&order.country) else { continue };
        match eco.cancel_trade_order(order.order_index) {
            Ok(done) => println!("[ТОРГОВЛЯ] {}: отменён заказ {}", order.country, trade_order_text(&done)),
            Err(reason) => println!("[ТОРГОВЛЯ] {}: отмена отклонена: {reason}", order.country),
        }
    }
    enforce_export_quotas(&mut economies.0);
    recompute_trade_links(&mut economies.0);
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

/// Гражданские фабрики для верхней панели: `Гражд. фабрики: 100 (Свободно: 80, ТНП: 20, Торговля: 0)`.
pub fn civilian_text(eco: &CountryEconomy) -> String {
    format!(
        "Гражд. фабрики: {} (Свободно: {}, ТНП: {}, Торговля: {})",
        eco.civilian_factories,
        eco.available_civilian_factories(),
        eco.consumer_goods_factories(),
        eco.dedicated_trade_factories()
    )
}

/// Строка проекта очереди: `Военный завод в стейте #5 [30/7200] (6 фабр.)`.
pub fn project_text(project: &ConstructionProject, factories: u32) -> String {
    format!(
        "{} в стейте #{} [{}/{}] ({factories} фабр.)",
        project.building_type.label(),
        project.state_id,
        project.progress,
        project.cost
    )
}

/// Строка эффективности линии: `Эфф: 20% (макс 50%)`.
pub fn efficiency_text(line: &ProductionLine) -> String {
    format!("Эфф: {}% (макс {}%)", line.efficiency, line.max_efficiency)
}

/// Плашка сырья: `[Ресурсы: ⚙️ 45/60 | 🪨 12 | ✈️ 20 | 🌿 5 | 🛢️ 8]` (сталь: свободно/всего, остальное: свободно).
pub fn resources_text(eco: &CountryEconomy) -> String {
    format!(
        "[Ресурсы: ⚙️ {}/{} | 🪨 {} | ✈️ {} | 🌿 {} | 🛢️ {}]",
        eco.resource_free(ResourceType::Steel),
        eco.total_resource(ResourceType::Steel),
        eco.resource_free(ResourceType::Tungsten),
        eco.resource_free(ResourceType::Aluminium),
        eco.resource_free(ResourceType::Rubber),
        eco.resource_free(ResourceType::Oil)
    )
}

/// Статус сырья линии: `Сырьё: 100%` или `Дефицит Стали: -45% к выпуску`.
pub fn resource_status_text(eco: &CountryEconomy, equipment: EquipmentType) -> String {
    match eco.resource_shortage(equipment) {
        Some((resource, percent)) => format!("Дефицит {}: -{percent}% к выпуску", resource.genitive()),
        None => "Сырьё: 100%".to_string(),
    }
}

/// Строка контракта: `+24 Сталь от SWE [3 фабр.]`.
pub fn trade_order_text(order: &TradeOrder) -> String {
    format!("+{} {} от {} [{} фабр.]", order.amount(), order.resource.label(), order.seller, order.factories)
}

/// Состояние формы создания сделки.
struct TradeForm {
    resource: ResourceType,
    seller: String,
    factories: u32,
}

impl Default for TradeForm {
    fn default() -> Self {
        Self { resource: ResourceType::Steel, seller: String::new(), factories: 1 }
    }
}

/// Плашка «Торговля»: активные поставки с кнопкой отмены и форма нового заказа.
fn trade_panel(
    mut contexts: EguiContexts,
    economies: Res<Economies>,
    player: Res<PlayerCountry>,
    mut form: Local<TradeForm>,
    mut creates: EventWriter<CreateTradeOrder>,
    mut cancels: EventWriter<CancelTradeOrder>,
) {
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    let Some(eco) = economies.0.get(&player.0) else { return };
    let frame = egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 200));
    egui::Window::new("Торговля")
        .anchor(egui::Align2::LEFT_TOP, [10.0, 52.0])
        .resizable(false)
        .collapsible(true)
        .frame(frame)
        .show(ctx, |ui| {
            if eco.trade_orders.is_empty() {
                ui.label("Нет активных поставок");
            }
            for (index, order) in eco.trade_orders.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(trade_order_text(order));
                    if ui.button("Отмена").clicked() {
                        cancels.send(CancelTradeOrder { country: player.0.clone(), order_index: index });
                    }
                });
            }
            if eco.imported_factories > 0 {
                ui.label(format!("Получено фабрик от покупателей: {}", eco.imported_factories));
            }
            ui.separator();
            egui::ComboBox::from_label("Ресурс").selected_text(form.resource.label()).show_ui(ui, |ui| {
                for resource in ResourceType::ALL {
                    ui.selectable_value(&mut form.resource, resource, resource.label());
                }
            });
            ui.horizontal(|ui| {
                ui.label("Продавец:");
                ui.add(egui::TextEdit::singleline(&mut form.seller).desired_width(48.0));
            });
            let free = eco.own_available_civilian_factories();
            let seller = form.seller.trim().to_uppercase();
            let (free_export_units, total_export) = free_export(&economies.0, &seller, form.resource);
            ui.label(format!("Доступно для импорта: {free_export_units}/{total_export} ед."));
            ui.horizontal(|ui| {
                if ui.add_enabled(form.factories > 1, egui::Button::new("−")).clicked() {
                    form.factories -= 1;
                }
                ui.label(format!("Фабрик: {} (+{} ед.)", form.factories, form.factories * RESOURCE_PER_FACTORY));
                if ui.add_enabled(form.factories < free, egui::Button::new("+")).clicked() {
                    form.factories += 1;
                }
            });
            let valid = !seller.is_empty()
                && form.factories >= 1
                && form.factories <= free
                && form.factories * RESOURCE_PER_FACTORY <= free_export_units;
            if ui.add_enabled(valid, egui::Button::new("Купить")).clicked() {
                creates.send(CreateTradeOrder {
                    country: player.0.clone(),
                    resource: form.resource,
                    seller: form.seller.trim().to_uppercase(),
                    factories: form.factories,
                });
            }
        });
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
                        "🏭 {} | 🔨 Военные заводы: {} | {} | {} | {}",
                        civilian_text(eco),
                        eco.military_factories,
                        resources_text(eco),
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
    mut switch_orders: EventWriter<SwitchProductionLine>,
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
                    // эффективность линии, если она существует; смена типа сбрасывает её до 10%
                    if let Some(line) = eco.production_lines.iter().find(|l| l.equipment == equipment) {
                        ui.label(efficiency_text(line));
                        ui.label(resource_status_text(eco, equipment));
                        ui.menu_button("тип", |ui| {
                            for target in EquipmentType::ALL {
                                let taken = eco.production_lines.iter().any(|l| l.equipment == target && l.assigned_factories > 0);
                                if !taken && target != equipment && ui.button(format!("-> {} (эфф. {SWITCH_EFFICIENCY}%)", target.label())).clicked() {
                                    switch_orders.send(SwitchProductionLine {
                                        country: player.0.clone(),
                                        from: equipment,
                                        to: target,
                                    });
                                    ui.close_menu();
                                }
                            }
                        });
                    }
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
            if let Some(state) = state {
                ui.label(format!("Гражданских фабрик в стейте: {}", state_factories.civilian_in(state.id)));
            }
            // кнопка доступна, только если проект допустим (can_queue): стейт под контролем, есть что конвертировать
            let buttons = [
                (BuildingType::MilitaryFactory, "+ Построить военный завод в стейте"),
                (BuildingType::CivToMilConversion, "Конвертировать Civ -> Mil"),
                (BuildingType::MilToCivConversion, "Конвертировать Mil -> Civ"),
            ];
            for (kind, caption) in buttons {
                let allowed = state.is_some_and(|s| can_queue(kind, s, &controllers, eco, &state_factories, &player.0).is_ok());
                if ui.add_enabled(allowed, egui::Button::new(caption)).clicked() {
                    if let Some(state) = state {
                        orders.send(QueueConstruction { country: player.0.clone(), state_id: state.id, building_type: kind });
                    }
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
                resources: Default::default(),
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
        let mut economy = CountryEconomy::new_abundant(10, 10);
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
        let mut economy = CountryEconomy::new_abundant(10, 10);
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
        let eco = CountryEconomy::new_abundant(5, 10);
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
            let eco = CountryEconomy::new_abundant(0, mil);
            let assigned: u32 = eco.production_lines.iter().map(|l| l.assigned_factories).sum();
            assert_eq!(assigned, mil, "{mil} заводов распределены без остатка");
            assert_eq!(eco.free_factories(), 0);
        }
        // 10 заводов: 15% -> 1 пушки, 25% -> 2 лёгких танка, 10% -> 1 оснащения, остаток до 10 — винтовки (6)
        let eco = CountryEconomy::new_abundant(0, 10);
        let lines: BTreeMap<_, _> = eco.production_lines.iter().map(|l| (l.equipment, l.assigned_factories)).collect();
        assert_eq!(lines[&InfantryEquipment], 6);
        assert_eq!((lines[&SupportEquipment], lines[&Artillery], lines[&LightTank], lines[&MediumTank]), (1, 1, 2, 0));
        assert_eq!((eco.output_of(InfantryEquipment), eco.output_of(LightTank), eco.output_of(Artillery)), (6, 1, 1));
        // винтовки: 6 заводов * 5 * 20% = 6; танки и орудия дают меньше 1 -> действует минимум 1 в сутки
    }

    #[test]
    fn assign_factories_respects_the_military_factory_limit() {
        let mut eco = CountryEconomy::new_abundant(0, 10);
        // заводы все заняты: на средние танки свободных нет
        assert_eq!(eco.assign_factories(MediumTank, 3), 0);
        // освобождаем 4 завода на винтовках — теперь 3 уходят в средние танки, ещё 1 свободен
        assert_eq!(eco.assign_factories(InfantryEquipment, 2), 2);
        assert_eq!(eco.free_factories(), 4);
        assert_eq!(eco.assign_factories(MediumTank, 3), 3);
        assert_eq!(eco.free_factories(), 1);
        // 3 завода * 1 * 20% = 0 -> минимум 1 в сутки
        assert_eq!(eco.output_of(MediumTank), 1);
        // запрос больше лимита обрезается до свободного (1 свободный + 3 уже на линии)
        assert_eq!(eco.assign_factories(MediumTank, 100), 4);
        assert_eq!(eco.free_factories(), 0);
        // общая сумма заводов на линиях не больше лимита
        let total: u32 = eco.production_lines.iter().map(|l| l.assigned_factories).sum();
        assert!(total <= eco.military_factories);
    }

    #[test]
    fn oversubscribed_lines_are_clamped_when_producing() {
        let mut eco = CountryEconomy::new_abundant(0, 10);
        eco.production_lines = vec![
            ProductionLine::new(InfantryEquipment, 8),
            ProductionLine::new(LightTank, 8),
        ];
        eco.recalc_daily_output();
        // винтовки заняли 8 из 10; на танки остаётся 2
        assert_eq!(eco.effective_factories(), vec![(InfantryEquipment, 8), (LightTank, 2)]);
        assert_eq!((eco.output_of(InfantryEquipment), eco.output_of(LightTank)), (8, 1));
        assert_eq!(eco.free_factories(), 0);
    }

    fn production_app() -> App {
        let mut app = App::new();
        let mut eco = CountryEconomy::new_abundant(0, 10);
        for t in EquipmentType::ALL {
            eco.set_stock(t, 0);
        }
        app.add_event::<DailyTickEvent>()
            .add_event::<SetProductionLine>()
            .add_event::<QueueConstruction>()
            .add_event::<SwitchProductionLine>()
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
        // линии 10 заводов при эффективности 20%: винтовки 6 * 5 * 20% = 6; у оснащения, орудий и лёгких
        // танков получается меньше единицы, поэтому действует минимум 1; средние танки — без заводов
        assert_eq!(eco.stock(InfantryEquipment), 6);
        assert_eq!(eco.stock(SupportEquipment), 1);
        assert_eq!(eco.stock(Artillery), 1);
        assert_eq!(eco.stock(LightTank), 1);
        assert_eq!(eco.stock(MediumTank), 0);
        day(&mut app);
        // второй день идёт при эффективности 21%: 30 * 21 / 100 = 6
        assert_eq!(app.world().resource::<Economies>().0["GER"].stock(InfantryEquipment), 12, "выход накапливается");
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
        assert_eq!(eco.output_of(MediumTank), 1, "6 -> 2 освободило 4 завода, но 4 * 20% < 1 -> минимум 1");
        let total: u32 = eco.production_lines.iter().map(|l| l.assigned_factories).sum();
        assert_eq!(total, 10);
        // неизвестная страна приказ игнорирует без паники
        app.world_mut().send_event(SetProductionLine { country: "XXX".into(), equipment: Artillery, factories: 1 });
        app.update();
        day(&mut app);
        assert_eq!(app.world().resource::<Economies>().0["GER"].stock(MediumTank), 1);
    }

    #[test]
    fn top_panel_shows_rifles_tanks_and_guns() {
        let mut eco = CountryEconomy::new_abundant(0, 10);
        eco.set_stock(InfantryEquipment, 1234);
        eco.set_stock(LightTank, 40);
        eco.set_stock(MediumTank, 2);
        eco.set_stock(Artillery, 77);
        eco.reinforce_cost.insert(InfantryEquipment, 50);
        assert_eq!(stockpile_text(&eco), "📦 Винтовки: 1234 (+6 / -50 день) | Танки: 42 | Пушки: 77");
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
            let eco = CountryEconomy::new_abundant(total, 0);
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
            resources: Default::default(),
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
            .insert_resource(StateFactories { military: state_factories.iter().copied().collect(), ..Default::default() })
            .add_systems(Update, daily_construction);
        app
    }

    #[test]
    fn completed_factory_joins_the_state_and_the_nation_and_can_be_assigned() {
        // 10 гражданских: 2 на ТНП, 8 свободных; проекту не хватает 5 очков — нужна одна фабрика
        let mut economy = CountryEconomy::new_abundant(10, 10);
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
        let mut economy = CountryEconomy::new_abundant(10, 10);
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
        let mut economy = CountryEconomy::new_abundant(1, 10);
        economy.construction.projects.push(project(5, 0));
        let mut app = daily_app(economy, &[]);
        day(&mut app);
        assert_eq!(app.world().resource::<Economies>().0["GER"].construction.projects[0].progress, 0);
    }

    fn order_app() -> App {
        let mut app = App::new();
        app.add_event::<QueueConstruction>()
            .init_resource::<StateFactories>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), CountryEconomy::new_abundant(10, 10))])))
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
        app.world_mut().send_event(QueueConstruction {
            country: country.into(),
            state_id,
            building_type: BuildingType::MilitaryFactory,
        });
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
        let eco = CountryEconomy::new_abundant(100, 10);
        assert_eq!(civilian_text(&eco), "Гражд. фабрики: 100 (Свободно: 80, ТНП: 20, Торговля: 0)");
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

#[cfg(test)]
mod efficiency_tests {
    use super::*;

    use EquipmentType::*;

    fn line(equipment: EquipmentType, factories: u32, efficiency: u32) -> ProductionLine {
        ProductionLine { efficiency, ..ProductionLine::new(equipment, factories) }
    }

    #[test]
    fn new_lines_start_at_twenty_percent_with_a_fifty_percent_cap() {
        let l = ProductionLine::new(Artillery, 3);
        assert_eq!((l.efficiency, l.max_efficiency), (20, 50));
        // стартовые линии страны тоже начинают с 20%
        let eco = CountryEconomy::new_abundant(0, 10);
        assert!(eco.production_lines.iter().all(|l| (l.efficiency, l.max_efficiency) == (20, 50)));
    }

    #[test]
    fn output_is_factories_times_base_times_efficiency() {
        // 6 заводов * 5 винтовок * эффективность / 100
        assert_eq!(line(InfantryEquipment, 6, 20).output(6), 6);
        assert_eq!(line(InfantryEquipment, 6, 50).output(6), 15);
        assert_eq!(line(InfantryEquipment, 6, 100).output(6), 30);
        assert_eq!(line(InfantryEquipment, 6, 33).output(6), 9, "30 * 33 / 100 = 9, округление вниз");
        // орудия: 4 * 1 * 50% = 2; при 20% получается 0, но назначенные заводы дают минимум 1
        assert_eq!(line(Artillery, 4, 50).output(4), 2);
        assert_eq!(line(Artillery, 4, 20).output(4), 1);
        assert_eq!(line(LightTank, 1, 10).output(1), 1);
        // без работающих заводов выпуска нет (минимум 1 действует только при назначенных)
        assert_eq!(line(LightTank, 0, 100).output(0), 0);
        assert_eq!(line(LightTank, 5, 100).output(0), 0, "назначено, но лимит не оставил заводов");
    }

    #[test]
    fn efficiency_grows_one_percent_a_day_up_to_the_cap() {
        let mut l = ProductionLine::new(InfantryEquipment, 1);
        for day in 1..=30 {
            l.grow();
            assert_eq!(l.efficiency, 20 + day);
        }
        assert_eq!(l.efficiency, 50);
        l.grow();
        assert_eq!(l.efficiency, 50, "выше потолка не растёт");
        // повышенный потолок разрешает дальше
        l.max_efficiency = 100;
        l.grow();
        assert_eq!(l.efficiency, 51);
        // эффективность, оказавшаяся выше потолка, не откатывается назад
        l.efficiency = 60;
        l.max_efficiency = 50;
        l.grow();
        assert_eq!(l.efficiency, 60);
    }

    fn production_app() -> App {
        let mut app = App::new();
        let mut eco = CountryEconomy::new_abundant(0, 10);
        for t in EquipmentType::ALL {
            eco.set_stock(t, 0);
        }
        app.add_event::<DailyTickEvent>()
            .add_event::<SwitchProductionLine>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), eco)])))
            .add_systems(Update, (apply_switch_orders, daily_production.after(apply_switch_orders)));
        app
    }

    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    fn eco(app: &App) -> &CountryEconomy {
        &app.world().resource::<Economies>().0["GER"]
    }

    fn line_of(app: &App, equipment: EquipmentType) -> Option<&ProductionLine> {
        eco(app).production_lines.iter().find(|l| l.equipment == equipment)
    }

    #[test]
    fn daily_tick_produces_at_current_efficiency_then_grows_it() {
        let mut app = production_app();
        // сутки 1-3 идут при 20%, 21%, 22%: 30 * 20 / 100 = 6, 30 * 21 / 100 = 6, 30 * 22 / 100 = 6
        for _ in 0..3 {
            day(&mut app);
        }
        assert_eq!(eco(&app).stock(InfantryEquipment), 18);
        assert_eq!(line_of(&app, InfantryEquipment).unwrap().efficiency, 23, "после трёх суток +3%");
        // выпуск, показанный в интерфейсе, уже пересчитан под новую эффективность: 30 * 23 / 100 = 6
        assert_eq!(eco(&app).output_of(InfantryEquipment), 6);
        // через 30 суток линия на потолке 50% и даёт 30 * 50 / 100 = 15 в сутки
        for _ in 0..27 {
            day(&mut app);
        }
        assert_eq!(line_of(&app, InfantryEquipment).unwrap().efficiency, 50);
        assert_eq!(eco(&app).output_of(InfantryEquipment), 15);
        let before = eco(&app).stock(InfantryEquipment);
        day(&mut app);
        assert_eq!(eco(&app).stock(InfantryEquipment) - before, 15);
        assert_eq!(line_of(&app, InfantryEquipment).unwrap().efficiency, 50, "потолок держится");
    }

    #[test]
    fn switching_equipment_resets_efficiency_to_ten_percent() {
        let mut economy = CountryEconomy::new_abundant(0, 10);
        economy.production_lines.iter_mut().for_each(|l| l.efficiency = 45);
        // винтовки (6 заводов) -> средние танки: у средних танков пустая линия, она заменяется
        assert_eq!(economy.switch_line_equipment(InfantryEquipment, MediumTank), Ok(()));
        let medium = economy.production_lines.iter().find(|l| l.equipment == MediumTank).unwrap();
        assert_eq!((medium.assigned_factories, medium.efficiency, medium.max_efficiency), (6, 10, 50));
        assert!(economy.production_lines.iter().all(|l| l.equipment != InfantryEquipment));
        assert_eq!(economy.production_lines.iter().filter(|l| l.equipment == MediumTank).count(), 1, "дубликатов линий нет");
        // 6 заводов * 1 * 10% = 0 -> минимум 1
        assert_eq!(economy.output_of(MediumTank), 1);
        // остальные линии свою эффективность сохраняют
        assert!(economy.production_lines.iter().filter(|l| l.equipment != MediumTank).all(|l| l.efficiency == 45));
        // общий лимит заводов остался прежним
        assert_eq!(economy.free_factories(), 0);
    }

    #[test]
    fn switching_is_rejected_for_missing_same_or_busy_target_lines() {
        let mut economy = CountryEconomy::new_abundant(0, 10);
        assert_eq!(economy.switch_line_equipment(InfantryEquipment, InfantryEquipment), Err("тип не меняется"));
        // пушки уже работают (1 завод)
        assert_eq!(economy.switch_line_equipment(InfantryEquipment, Artillery), Err("линия этого типа уже работает"));
        // линии средних танков не было бы после замены — а исходной линии нет вовсе
        economy.production_lines.retain(|l| l.equipment != LightTank);
        assert_eq!(economy.switch_line_equipment(LightTank, MediumTank), Err("такой линии нет"));
        // отклонение ничего не меняет
        assert!(economy.production_lines.iter().all(|l| l.efficiency == 20));
    }

    #[test]
    fn switch_orders_work_through_events_and_efficiency_climbs_again() {
        let mut app = production_app();
        for _ in 0..10 {
            day(&mut app);
        }
        assert_eq!(line_of(&app, InfantryEquipment).unwrap().efficiency, 30);
        app.world_mut().send_event(SwitchProductionLine { country: "GER".into(), from: InfantryEquipment, to: MediumTank });
        app.update();
        assert_eq!(line_of(&app, MediumTank).unwrap().efficiency, 10, "смена типа сбросила эффективность");
        assert!(line_of(&app, InfantryEquipment).is_none());
        // дальше линия снова набирает по 1% в сутки
        day(&mut app);
        day(&mut app);
        assert_eq!(line_of(&app, MediumTank).unwrap().efficiency, 12);
        // приказ для неизвестной страны или с недопустимой сменой не паникует и ничего не меняет
        app.world_mut().send_event(SwitchProductionLine { country: "XXX".into(), from: Artillery, to: LightTank });
        app.world_mut().send_event(SwitchProductionLine { country: "GER".into(), from: Artillery, to: LightTank });
        app.update();
        assert!(line_of(&app, Artillery).is_some());
    }

    #[test]
    fn efficiency_text_matches_the_panel_format() {
        assert_eq!(efficiency_text(&ProductionLine::new(Artillery, 1)), "Эфф: 20% (макс 50%)");
        assert_eq!(efficiency_text(&line(LightTank, 1, 37)), "Эфф: 37% (макс 50%)");
    }
}

#[cfg(test)]
mod conversion_tests {
    use super::*;
    use std::sync::Arc;

    use crate::map::StateDef;
    use EquipmentType::*;

    fn state(id: u32, provinces: Vec<u32>) -> Arc<StateDef> {
        Arc::new(StateDef {
            id,
            name: format!("S{id}"),
            owner: "GER".into(),
            provinces,
            victory_points: vec![],
            civilian_factories: 0,
            military_factories: 0,
            resources: Default::default(),
        })
    }

    fn controllers() -> ProvinceControllers {
        ProvinceControllers(BTreeMap::from([
            (1, "GER".to_string()),
            (2, "GER".to_string()),
            (3, "FRA".to_string()),
            (4, "FRA".to_string()),
        ]))
    }

    fn factories(military: &[(u32, u32)], civilian: &[(u32, u32)]) -> StateFactories {
        StateFactories { military: military.iter().copied().collect(), civilian: civilian.iter().copied().collect() }
    }

    #[test]
    fn building_types_have_the_requested_costs_and_names() {
        assert_eq!(BuildingType::MilitaryFactory.cost(), 7200);
        assert_eq!(BuildingType::CivToMilConversion.cost(), 4000);
        assert_eq!(BuildingType::MilToCivConversion.cost(), 4000);
        assert_eq!(ConstructionProject::new(BuildingType::CivToMilConversion, 5).cost, 4000);
        assert_eq!(
            project_text(&ConstructionProject::new(BuildingType::CivToMilConversion, 5), 3),
            "Конверсия Civ -> Mil в стейте #5 [0/4000] (3 фабр.)"
        );
        assert_eq!(
            project_text(&ConstructionProject::new(BuildingType::MilToCivConversion, 9), 15),
            "Конверсия Mil -> Civ в стейте #9 [0/4000] (15 фабр.)"
        );
    }

    #[test]
    fn a_conversion_takes_fifty_four_days_with_fifteen_factories() {
        let mut queue = ConstructionQueue { projects: vec![ConstructionProject::new(BuildingType::CivToMilConversion, 1)] };
        for day in 1..=53 {
            assert!(queue.advance(15).is_empty(), "на сутки {day} ещё не готово");
        }
        assert_eq!(queue.projects[0].progress, 3975);
        assert_eq!(queue.advance(15).len(), 1, "54 суток * 75 очков = 4050 >= 4000");
    }

    #[test]
    fn can_queue_checks_control_and_available_factories() {
        let s = state(1, vec![1, 2, 3]); // два из трёх под GER
        let foreign = state(2, vec![3, 4]);
        let eco = CountryEconomy::new_abundant(10, 10);
        let sf = factories(&[(1, 2)], &[(1, 3)]);
        let c = controllers();
        let ok = |kind, st: &StateDef| can_queue(kind, st, &c, &eco, &sf, "GER");
        assert_eq!(ok(BuildingType::MilitaryFactory, &s), Ok(()));
        assert_eq!(ok(BuildingType::CivToMilConversion, &s), Ok(()));
        assert_eq!(ok(BuildingType::MilToCivConversion, &s), Ok(()));
        // чужой стейт — ничего нельзя
        for kind in [BuildingType::MilitaryFactory, BuildingType::CivToMilConversion, BuildingType::MilToCivConversion] {
            assert_eq!(ok(kind, &foreign), Err("страна не контролирует стейт"));
        }
        // в стейте без фабрик конвертировать нечего
        let empty = factories(&[], &[]);
        assert!(can_queue(BuildingType::CivToMilConversion, &s, &c, &eco, &empty, "GER").is_err());
        assert!(can_queue(BuildingType::MilToCivConversion, &s, &c, &eco, &empty, "GER").is_err());
        // строить новый военный завод можно и в стейте без заводов
        assert_eq!(can_queue(BuildingType::MilitaryFactory, &s, &c, &eco, &empty, "GER"), Ok(()));
    }

    #[test]
    fn queued_conversions_reserve_the_factories_they_will_use() {
        let s = state(1, vec![1, 2]);
        let c = controllers();
        let sf = factories(&[(1, 1)], &[(1, 2)]);
        let mut eco = CountryEconomy::new_abundant(10, 10);
        // в стейте две гражданские фабрики: третья конверсия уже не пройдёт
        for _ in 0..2 {
            assert_eq!(can_queue(BuildingType::CivToMilConversion, &s, &c, &eco, &sf, "GER"), Ok(()));
            eco.construction.projects.push(ConstructionProject::new(BuildingType::CivToMilConversion, 1));
        }
        assert!(can_queue(BuildingType::CivToMilConversion, &s, &c, &eco, &sf, "GER").is_err());
        // а вот военный завод в стейте один: после первой конверсии вторая не принимается
        assert_eq!(can_queue(BuildingType::MilToCivConversion, &s, &c, &eco, &sf, "GER"), Ok(()));
        eco.construction.projects.push(ConstructionProject::new(BuildingType::MilToCivConversion, 1));
        assert!(can_queue(BuildingType::MilToCivConversion, &s, &c, &eco, &sf, "GER").is_err());
        // у страны всего две гражданские фабрики: третьей для конверсии нет и в другом стейте
        let mut small = CountryEconomy::new_abundant(2, 10);
        small.construction.projects.extend([
            ConstructionProject::new(BuildingType::CivToMilConversion, 7),
            ConstructionProject::new(BuildingType::CivToMilConversion, 7),
        ]);
        let other = state(2, vec![1, 2]);
        let sf2 = factories(&[], &[(2, 5)]);
        assert_eq!(
            can_queue(BuildingType::CivToMilConversion, &other, &c, &small, &sf2, "GER"),
            Err("у страны нет гражданских фабрик для конверсии")
        );
    }

    fn daily_app(economy: CountryEconomy, sf: StateFactories) -> App {
        let mut app = App::new();
        app.add_event::<DailyTickEvent>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), economy)])))
            .insert_resource(sf)
            .add_systems(Update, daily_construction);
        app
    }

    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    fn almost_done(kind: BuildingType, state_id: u32) -> ConstructionProject {
        ConstructionProject { progress: kind.cost() - 5, ..ConstructionProject::new(kind, state_id) }
    }

    #[test]
    fn civilian_factory_becomes_a_military_one() {
        let mut economy = CountryEconomy::new_abundant(10, 10);
        economy.construction.projects.push(almost_done(BuildingType::CivToMilConversion, 5));
        let mut app = daily_app(economy, factories(&[(5, 1)], &[(5, 4)]));
        day(&mut app);
        let eco = &app.world().resource::<Economies>().0["GER"];
        assert_eq!((eco.civilian_factories, eco.military_factories), (9, 11));
        assert!(eco.construction.projects.is_empty());
        let sf = app.world().resource::<StateFactories>();
        assert_eq!((sf.military_in(5), sf.civilian_in(5)), (2, 3));
        // новый завод свободен для линий; ТНП пересчитаны под 9 фабрик (1), свободных гражданских 8
        assert_eq!(eco.free_factories(), 1);
        assert_eq!((eco.consumer_goods_factories(), eco.available_civilian_factories()), (1, 8));
    }

    #[test]
    fn military_factory_becomes_a_civilian_one_and_lines_stay_within_the_limit() {
        let mut economy = CountryEconomy::new_abundant(10, 10);
        economy.construction.projects.push(almost_done(BuildingType::MilToCivConversion, 5));
        // все 10 заводов заняты линиями: конверсия снимет завод с последней работающей линии (лёгкие танки)
        let before: u32 = economy.production_lines.iter().map(|l| l.assigned_factories).sum();
        assert_eq!(before, 10);
        let mut app = daily_app(economy, factories(&[(5, 2)], &[(5, 1)]));
        day(&mut app);
        let eco = &app.world().resource::<Economies>().0["GER"];
        assert_eq!((eco.civilian_factories, eco.military_factories), (11, 9));
        let sf = app.world().resource::<StateFactories>();
        assert_eq!((sf.military_in(5), sf.civilian_in(5)), (1, 2));
        let assigned: u32 = eco.production_lines.iter().map(|l| l.assigned_factories).sum();
        assert_eq!(assigned, 9, "назначения не превышают новый лимит");
        let light_tanks = eco.production_lines.iter().find(|l| l.equipment == LightTank).unwrap();
        assert_eq!(light_tanks.assigned_factories, 1, "снят завод с последней работающей линии");
        assert_eq!(eco.free_factories(), 0);
    }

    #[test]
    fn free_military_factory_is_converted_before_any_line_loses_one() {
        let mut economy = CountryEconomy::new_abundant(10, 10);
        economy.assign_factories(InfantryEquipment, 3); // 3 заводов освободилось
        assert_eq!(economy.free_factories(), 3);
        economy.construction.projects.push(almost_done(BuildingType::MilToCivConversion, 5));
        let lines_before: Vec<u32> = economy.production_lines.iter().map(|l| l.assigned_factories).collect();
        let mut app = daily_app(economy, factories(&[(5, 1)], &[]));
        day(&mut app);
        let eco = &app.world().resource::<Economies>().0["GER"];
        assert_eq!(eco.production_lines.iter().map(|l| l.assigned_factories).collect::<Vec<_>>(), lines_before);
        assert_eq!((eco.military_factories, eco.free_factories()), (9, 2));
    }

    #[test]
    fn conversion_without_a_factory_at_completion_is_cancelled_without_effect() {
        let mut economy = CountryEconomy::new_abundant(10, 10);
        economy.construction.projects.extend([
            almost_done(BuildingType::CivToMilConversion, 5),
            almost_done(BuildingType::MilToCivConversion, 6),
        ]);
        // в стейте 5 нет гражданских, в стейте 6 нет военных (фабрики пропали, например, при захвате)
        let mut app = daily_app(economy, factories(&[(5, 1)], &[(6, 1)]));
        day(&mut app);
        let eco = &app.world().resource::<Economies>().0["GER"];
        assert_eq!((eco.civilian_factories, eco.military_factories), (10, 10), "ничего не изменилось");
        assert!(eco.construction.projects.is_empty(), "проекты сняты из очереди");
        let sf = app.world().resource::<StateFactories>();
        assert_eq!((sf.military_in(5), sf.civilian_in(5), sf.military_in(6), sf.civilian_in(6)), (1, 0, 0, 1));
    }

    #[test]
    fn conversion_orders_go_through_the_queue_system() {
        let mut app = App::new();
        app.add_event::<QueueConstruction>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), CountryEconomy::new_abundant(10, 10))])))
            .insert_resource(StateRegistry { states: vec![state(1, vec![1, 2]), state(2, vec![3, 4])], ..Default::default() })
            .insert_resource(controllers())
            .insert_resource(factories(&[(1, 1), (2, 1)], &[(1, 2), (2, 2)]))
            .add_systems(Update, apply_construction_orders);
        let mut order = |kind: BuildingType, state_id: u32| {
            app.world_mut().send_event(QueueConstruction { country: "GER".into(), state_id, building_type: kind });
            app.update();
        };
        order(BuildingType::CivToMilConversion, 1);
        order(BuildingType::MilToCivConversion, 1);
        order(BuildingType::CivToMilConversion, 2); // чужой стейт
        order(BuildingType::MilToCivConversion, 1); // единственный военный завод уже зарезервирован
        let projects = &app.world().resource::<Economies>().0["GER"].construction.projects;
        assert_eq!(projects.len(), 2);
        assert_eq!((projects[0].building_type, projects[0].state_id, projects[0].cost), (BuildingType::CivToMilConversion, 1, 4000));
        assert_eq!((projects[1].building_type, projects[1].state_id, projects[1].cost), (BuildingType::MilToCivConversion, 1, 4000));
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    use EquipmentType::*;
    use ResourceType::*;

    /// 100 заводов: линии 50 / 10 / 15 / 25 / 0 (пехота, оснащение, орудия, лёгкие танки, средние).
    fn ger() -> CountryEconomy {
        let mut eco = CountryEconomy::new_abundant(0, 100);
        eco.set_pool(ger_start_resources());
        eco
    }

    #[test]
    fn start_pool_of_germany_matches_the_spec() {
        let pool = ger_start_resources();
        assert_eq!(
            pool.into_iter().collect::<Vec<_>>(),
            vec![(Steel, 60), (Tungsten, 15), (Aluminium, 20), (Rubber, 5), (Oil, 10)]
        );
    }

    #[test]
    fn cost_table_is_per_factory_per_day() {
        assert_eq!(InfantryEquipment.resource_cost(), &[(Steel, 1)]);
        assert_eq!(SupportEquipment.resource_cost(), &[(Steel, 1), (Aluminium, 1)]);
        assert_eq!(Artillery.resource_cost(), &[(Steel, 2), (Tungsten, 1)]);
        assert_eq!(LightTank.resource_cost(), &[(Steel, 2), (Tungsten, 1), (Oil, 1)]);
        assert_eq!(MediumTank.resource_cost(), &[(Steel, 3), (Tungsten, 2), (Oil, 1)]);
    }

    #[test]
    fn demand_sums_assigned_factories_times_cost() {
        let eco = ger();
        let demand = eco.resource_demand();
        assert_eq!(demand.get(&Steel), Some(&140), "50*1 + 10*1 + 15*2 + 25*2");
        assert_eq!(demand.get(&Tungsten), Some(&40), "15*1 + 25*1");
        assert_eq!(demand.get(&Aluminium), Some(&10));
        assert_eq!(demand.get(&Oil), Some(&25));
        assert_eq!(demand.get(&Rubber), None, "резина не нужна ни одной линии");
    }

    #[test]
    fn deficit_percent_is_integer_and_zero_when_covered() {
        assert_eq!(deficit_percent(60, 140), 57, "80 * 100 / 140 = 57.1 -> 57");
        assert_eq!(deficit_percent(15, 40), 62, "62.5 -> 62");
        assert_eq!(deficit_percent(45, 100), 55);
        assert_eq!(deficit_percent(0, 10), 100);
        assert_eq!(deficit_percent(10, 10), 0, "ровно хватает");
        assert_eq!(deficit_percent(11, 10), 0, "избыток");
        assert_eq!(deficit_percent(5, 0), 0, "нет потребности");
    }

    #[test]
    fn line_penalty_is_the_worst_deficit_among_its_resources() {
        let eco = ger();
        assert_eq!(eco.resource_penalty(InfantryEquipment), 57, "только сталь");
        assert_eq!(eco.resource_penalty(SupportEquipment), 57, "сталь 57 против алюминия 0");
        assert_eq!(eco.resource_penalty(Artillery), 62, "вольфрам 62 хуже стали 57");
        assert_eq!(eco.resource_penalty(LightTank), 62, "вольфрам 62, нефть 60, сталь 57");
        assert_eq!(eco.resource_shortage(LightTank), Some((Tungsten, 62)));
        assert_eq!(eco.resource_penalty(MediumTank), 62, "линия пуста, но потребность типа считается по общему дефициту");
    }

    #[test]
    fn no_deficit_when_the_pool_covers_demand() {
        let mut eco = ger();
        eco.set_pool(BTreeMap::from([(Steel, 140), (Tungsten, 40), (Aluminium, 10), (Oil, 25)]));
        for t in EquipmentType::ALL {
            assert_eq!(eco.resource_penalty(t), 0);
            assert_eq!(eco.resource_shortage(t), None);
        }
        assert_eq!(eco.output_of(InfantryEquipment), 50, "50 * 5 * 20% без штрафа");
        // страна без заданного пула не ограничена
        assert_eq!(CountryEconomy::new_abundant(0, 100).resource_penalty(LightTank), 0);
    }

    #[test]
    fn penalty_is_capped_at_ninety_percent() {
        let mut eco = CountryEconomy::new_abundant(0, 100);
        eco.production_lines = vec![ProductionLine::new(InfantryEquipment, 100)];
        eco.set_pool(BTreeMap::new());
        assert_eq!(eco.resource_deficits().get(&Steel), Some(&100), "сам дефицит 100%");
        assert_eq!(eco.resource_penalty(InfantryEquipment), MAX_RESOURCE_PENALTY, "штраф ограничен 90%");
        // база 100 * 5 * 20% = 100; 10% потенциала остаётся
        assert_eq!(eco.output_of(InfantryEquipment), 10);
        assert_eq!(resource_status_text(&eco, InfantryEquipment), "Дефицит Стали: -90% к выпуску");
        // и сама линия не принимает штраф выше потолка
        assert_eq!(ProductionLine::new(InfantryEquipment, 100).output_with_penalty(100, 100), 10);
    }

    #[test]
    fn output_with_penalty_keeps_the_minimum_of_one() {
        let line = ProductionLine::new(InfantryEquipment, 1); // база 1
        assert_eq!(line.output_with_penalty(1, 90), 1);
        assert_eq!(line.output_with_penalty(0, 90), 0, "без работающих заводов выпуска нет");
        let artillery = ProductionLine::new(Artillery, 15); // 15 * 20% = 3
        assert_eq!(artillery.output_with_penalty(15, 62), 1, "3 * 38 / 100 = 1");
        assert_eq!(artillery.output_with_penalty(15, 0), 3);
    }

    #[test]
    fn daily_output_drops_under_shortage() {
        let eco = ger();
        assert_eq!(eco.output_of(InfantryEquipment), 21, "база 50, штраф 57% -> 50 * 43 / 100");
        assert_eq!(eco.output_of(SupportEquipment), 2, "база 6 * 43 / 100");
        assert_eq!(eco.output_of(Artillery), 1, "база 3 * 38 / 100 = 1");
        assert_eq!(eco.output_of(LightTank), 1, "база 5 * 38 / 100 = 1");
    }

    #[test]
    fn midnight_tick_applies_the_penalty_to_the_stockpile() {
        let mut eco = ger();
        for t in EquipmentType::ALL {
            eco.set_stock(t, 0);
        }
        let mut app = App::new();
        app.add_event::<DailyTickEvent>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), eco)])))
            .add_systems(Update, daily_production);
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
        let eco = &app.world().resource::<Economies>().0["GER"];
        assert_eq!(eco.stock(InfantryEquipment), 21);
        assert_eq!(eco.stock(SupportEquipment), 2);
        assert_eq!(eco.stock(Artillery), 1);
        // пополнили сталь: штраф винтовок уходит сразу на следующих сутках
        app.world_mut().resource_mut::<Economies>().0.get_mut("GER").unwrap().set_pool(BTreeMap::from([
            (Steel, 1000),
            (Tungsten, 40),
            (Aluminium, 10),
            (Oil, 25),
        ]));
        app.world_mut().send_event(DailyTickEvent { day: 2, month: 1, year: 1936 });
        app.update();
        let eco = &app.world().resource::<Economies>().0["GER"];
        // эффективность выросла до 21%: 50 * 5 * 21 / 100 = 52 без штрафа
        assert_eq!(eco.stock(InfantryEquipment), 21 + 52);
    }

    #[test]
    fn ui_texts_show_resources_and_status() {
        let eco = ger();
        assert_eq!(resources_text(&eco), "[Ресурсы: ⚙️ 0/60 | 🪨 0 | ✈️ 10 | 🌿 5 | 🛢️ 0]");
        assert_eq!(resource_status_text(&eco, InfantryEquipment), "Дефицит Стали: -57% к выпуску");
        assert_eq!(resource_status_text(&eco, Artillery), "Дефицит Вольфрама: -62% к выпуску");
        let rich = CountryEconomy::new_abundant(0, 100);
        assert_eq!(resource_status_text(&rich, InfantryEquipment), "Сырьё: 100%");
        let mut covered = ger();
        covered.set_pool(BTreeMap::from([(Steel, 200), (Tungsten, 50), (Aluminium, 10), (Oil, 25), (Rubber, 5)]));
        assert_eq!(resources_text(&covered), "[Ресурсы: ⚙️ 60/200 | 🪨 10 | ✈️ 0 | 🌿 5 | 🛢️ 0]");
    }
}

#[cfg(test)]
mod trade_tests {
    use super::*;
    use EquipmentType::*;
    use ResourceType::*;

    /// 100 гражданских (20 ТНП, 80 свободно), 100 военных заводов на стандартных линиях, пул GER.
    fn buyer() -> CountryEconomy {
        let mut eco = CountryEconomy::new_abundant(100, 100);
        eco.set_pool(ger_start_resources());
        eco
    }

    fn trade_app(buyer: CountryEconomy, seller: CountryEconomy) -> App {
        let mut app = App::new();
        app.add_event::<DailyTickEvent>()
            .add_event::<CreateTradeOrder>()
            .add_event::<CancelTradeOrder>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), buyer), ("SWE".to_string(), seller)])))
            .add_systems(Update, (apply_trade_orders, daily_production.after(apply_trade_orders)));
        app
    }

    fn eco<'a>(app: &'a App, tag: &str) -> &'a CountryEconomy {
        &app.world().resource::<Economies>().0[tag]
    }

    fn buy(app: &mut App, resource: ResourceType, seller: &str, factories: u32) {
        app.world_mut().send_event(CreateTradeOrder { country: "GER".into(), resource, seller: seller.into(), factories });
        app.update();
    }

    #[test]
    fn rate_and_start_contract_match_the_spec() {
        assert_eq!(RESOURCE_PER_FACTORY, 8);
        assert_eq!(START_TRADE_SELLER, "SWE");
        assert_eq!(START_TRADE_FACTORIES * RESOURCE_PER_FACTORY, 24, "3 фабрики = 24 стали");
        let order = TradeOrder { resource: Steel, seller: "SWE".into(), factories: 3 };
        assert_eq!(order.amount(), 24);
        assert_eq!(trade_order_text(&order), "+24 Сталь от SWE [3 фабр.]");
    }

    #[test]
    fn order_takes_factories_from_available_not_from_consumer_goods() {
        let mut eco = buyer();
        assert_eq!((eco.consumer_goods_factories(), eco.available_civilian_factories()), (20, 80));
        eco.create_trade_order(Steel, "SWE", 3).unwrap();
        assert_eq!(eco.dedicated_trade_factories(), 3);
        assert_eq!(eco.available_civilian_factories(), 77, "100 - 20 ТНП - 3 торговля");
        assert_eq!(eco.consumer_goods_factories(), 20, "ТНП не тронуты");
        eco.create_trade_order(Oil, "USA", 7).unwrap();
        assert_eq!((eco.dedicated_trade_factories(), eco.available_civilian_factories()), (10, 70));
    }

    #[test]
    fn free_factories_never_go_negative() {
        let mut eco = CountryEconomy::new_abundant(10, 10); // 2 ТНП, 8 свободно
        assert!(eco.create_trade_order(Steel, "SWE", 9).is_err(), "9 > 8 свободных");
        assert!(eco.create_trade_order(Steel, "SWE", 0).is_err());
        eco.create_trade_order(Steel, "SWE", 8).unwrap();
        assert_eq!(eco.available_civilian_factories(), 0);
        assert!(eco.create_trade_order(Oil, "SWE", 1).is_err(), "свободных не осталось");
        assert_eq!(eco.trade_orders.len(), 1);
        assert_eq!(eco.consumer_goods_factories(), 2);
        // даже при потере фабрик (захват, конверсия) число не уходит в минус
        eco.civilian_factories = 3;
        assert_eq!(eco.available_civilian_factories(), 0);
    }

    #[test]
    fn construction_queue_gets_only_the_remaining_factories() {
        let mut eco = CountryEconomy::new_abundant(10, 0); // 8 свободно
        eco.create_trade_order(Steel, "SWE", 5).unwrap();
        eco.construction.projects.push(ConstructionProject::new(BuildingType::MilitaryFactory, 1));
        assert_eq!(eco.construction.allocation(eco.available_civilian_factories()), vec![3]);
    }

    #[test]
    fn imports_raise_the_effective_inflow_and_cut_the_penalty() {
        let mut eco = buyer();
        assert_eq!(eco.imported_resources(Steel), 0);
        assert_eq!(eco.resource_penalty(InfantryEquipment), 57, "60 стали против 140");
        assert_eq!(eco.output_of(InfantryEquipment), 21);
        eco.create_trade_order(Steel, "SWE", 3).unwrap();
        assert_eq!(eco.imported_resources(Steel), 24);
        assert_eq!(eco.total_resource(Steel), 84);
        assert_eq!(eco.resource(Steel), 60, "базовый пул не меняется");
        assert_eq!(eco.resource_penalty(InfantryEquipment), 40, "56 * 100 / 140 = 40");
        assert_eq!(eco.output_of(InfantryEquipment), 30, "50 * 60 / 100");
        // ещё 7 фабрик = +56 стали: суммарно 140, дефицита стали нет
        eco.create_trade_order(Steel, "NOR", 7).unwrap();
        assert_eq!(eco.imported_resources(Steel), 80);
        assert_eq!(eco.resource_penalty(InfantryEquipment), 0);
        assert_eq!(eco.output_of(InfantryEquipment), 50);
        assert_eq!(resource_status_text(&eco, InfantryEquipment), "Сырьё: 100%");
        // нефть не влияет на сталь, а вольфрамовые линии всё ещё страдают
        eco.create_trade_order(Oil, "USA", 1).unwrap();
        assert_eq!(eco.imported_resources(Steel), 80);
        assert_eq!(eco.resource_penalty(Artillery), 62);
        // отмена возвращает дефицит
        eco.cancel_trade_order(1).unwrap();
        assert_eq!(eco.resource_penalty(InfantryEquipment), 40);
    }

    #[test]
    fn seller_receives_the_buyers_factories() {
        let mut app = trade_app(buyer(), CountryEconomy::new_abundant(10, 0));
        assert_eq!(eco(&app, "SWE").available_civilian_factories(), 8);
        buy(&mut app, Steel, "SWE", 3);
        assert_eq!(eco(&app, "GER").available_civilian_factories(), 77);
        assert_eq!(eco(&app, "SWE").imported_factories, 3);
        assert_eq!(eco(&app, "SWE").available_civilian_factories(), 11, "8 своих + 3 переданных");
        assert_eq!(eco(&app, "SWE").consumer_goods_factories(), 2, "ТНП продавца не изменились");
        // отмена забирает фабрики обратно
        app.world_mut().send_event(CancelTradeOrder { country: "GER".into(), order_index: 0 });
        app.update();
        assert_eq!(eco(&app, "SWE").imported_factories, 0);
        assert_eq!(eco(&app, "SWE").available_civilian_factories(), 8);
        assert_eq!(eco(&app, "GER").available_civilian_factories(), 80);
    }

    #[test]
    fn invalid_orders_are_rejected_without_panic() {
        let mut app = trade_app(buyer(), CountryEconomy::new_abundant(10, 0));
        buy(&mut app, Steel, "XXX", 3); // нет такой страны
        buy(&mut app, Steel, "GER", 3); // сам себе
        buy(&mut app, Steel, "SWE", 81); // больше свободных (80)
        buy(&mut app, Steel, "SWE", 0);
        assert!(eco(&app, "GER").trade_orders.is_empty());
        assert_eq!(eco(&app, "GER").available_civilian_factories(), 80);
        app.world_mut().send_event(CancelTradeOrder { country: "GER".into(), order_index: 5 });
        app.world_mut().send_event(CancelTradeOrder { country: "NOP".into(), order_index: 0 });
        app.update();
        buy(&mut app, Steel, "SWE", 80);
        assert_eq!(eco(&app, "GER").available_civilian_factories(), 0);
    }

    #[test]
    fn midnight_tick_uses_imported_steel_and_refreshes_links() {
        let mut ger = buyer();
        for t in EquipmentType::ALL {
            ger.set_stock(t, 0);
        }
        ger.trade_orders.push(TradeOrder { resource: Steel, seller: "SWE".into(), factories: 10 }); // +80 стали
        let mut app = trade_app(ger, CountryEconomy::new_abundant(10, 0));
        assert_eq!(eco(&app, "SWE").imported_factories, 0, "связи обновятся в полночь");
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
        assert_eq!(eco(&app, "SWE").imported_factories, 10);
        // сталь 60 + 80 = 140 >= потребности: винтовки без штрафа, 50 * 5 * 20% = 50
        assert_eq!(eco(&app, "GER").stock(InfantryEquipment), 50);
        assert_eq!(eco(&app, "GER").stock(SupportEquipment), 6, "10 * 3 * 20% без штрафа (алюминий хватает)");
    }

    #[test]
    fn ui_text_shows_trade_factories() {
        let mut eco = CountryEconomy::new_abundant(100, 10);
        eco.create_trade_order(Steel, "SWE", 3).unwrap();
        assert_eq!(civilian_text(&eco), "Гражд. фабрики: 100 (Свободно: 77, ТНП: 20, Торговля: 3)");
        eco.set_pool(ger_start_resources());
        assert!(resources_text(&eco).starts_with("[Ресурсы: ⚙️ "));
    }
}

#[cfg(test)]
mod export_tests {
    use super::*;
    use crate::map::StateDef;
    use ResourceType::*;
    use std::sync::Arc;

    fn nat(steel: u32) -> BTreeMap<ResourceType, u32> {
        BTreeMap::from([(Steel, steel)])
    }

    fn eco_with(natural_steel: u32) -> CountryEconomy {
        let mut eco = CountryEconomy::new(100, 0);
        eco.set_natural_resources(nat(natural_steel));
        eco
    }

    /// GER и FRA (покупатели) и SWE (продавец) с добычей стали `swe_steel`.
    fn world(swe_steel: u32) -> BTreeMap<String, CountryEconomy> {
        BTreeMap::from([
            ("FRA".to_string(), eco_with(0)),
            ("GER".to_string(), eco_with(120)),
            ("SWE".to_string(), eco_with(swe_steel)),
        ])
    }

    #[test]
    fn export_focus_splits_production_fifty_fifty() {
        assert_eq!(ExportLaw::default(), ExportLaw::ExportFocus);
        assert_eq!(ExportLaw::ExportFocus.export_share_pct(), 50);
        let eco = eco_with(101);
        assert_eq!(eco.exportable_amount(Steel), 50, "101 * 50 / 100 вниз");
        assert_eq!(eco.domestic_available(Steel), 51, "остаток дома");
        assert_eq!(eco.exportable_amount(Steel) + eco.domestic_available(Steel), eco.natural(Steel));
        assert_eq!(eco.exportable_amount(Oil), 0, "нет добычи — нет экспорта");
        // у новой страны добычи нет вовсе: заглушки 1_000_000 больше нет
        let fresh = CountryEconomy::new(10, 10);
        assert!(fresh.natural_resources.is_empty());
        assert_eq!(fresh.resource(Steel), 0);
    }

    #[test]
    fn buyer_pool_is_domestic_plus_imports() {
        let mut ger = eco_with(120); // дома 60
        assert_eq!(ger.resource(Steel), 60);
        ger.create_trade_order(Steel, "SWE", 3).unwrap();
        assert_eq!(ger.total_resource(Steel), 84, "60 + 24");
        // потребность: 100 заводов не назначено — проверяем поток через деф. по прежней формуле
        ger.military_factories = 100;
        ger.production_lines = vec![ProductionLine::new(EquipmentType::InfantryEquipment, 100)];
        ger.recalc_daily_output();
        assert_eq!(ger.resource_penalty(EquipmentType::InfantryEquipment), 16, "(100 - 84) * 100 / 100");
    }

    #[test]
    fn sum_of_orders_cannot_exceed_seller_export_pool() {
        let mut w = world(100); // экспорт 50
        assert_eq!(free_export(&w, "SWE", Steel), (50, 50));
        open_trade_order(&mut w, "GER", Steel, "SWE", 4).unwrap(); // 32
        open_trade_order(&mut w, "FRA", Steel, "SWE", 2).unwrap(); // 16 -> 48
        assert_eq!(exported_from(&w, "SWE", Steel), 48);
        assert_eq!(free_export(&w, "SWE", Steel), (2, 50));
        // 1 фабрика = 8 > 2 свободных
        assert_eq!(open_trade_order(&mut w, "GER", Steel, "SWE", 1), Err("превышен экспортный лимит продавца"));
        assert_eq!(w["GER"].trade_orders.len(), 1, "отклонённый заказ не создаётся");
        assert_eq!(w["GER"].available_civilian_factories(), 100 - 20 - 4, "фабрики не списаны");
        // отмена освобождает квоту
        w.get_mut("FRA").unwrap().cancel_trade_order(0).unwrap();
        assert_eq!(free_export(&w, "SWE", Steel), (18, 50));
        open_trade_order(&mut w, "GER", Steel, "SWE", 2).unwrap(); // 16 <= 18
        // другой ресурс не затронут
        assert_eq!(free_export(&w, "SWE", Oil), (0, 0));
        assert!(open_trade_order(&mut w, "GER", Oil, "SWE", 1).is_err(), "нефти для экспорта нет");
    }

    #[test]
    fn order_exactly_at_the_limit_passes_and_foreign_sellers_fail() {
        let mut w = world(128); // экспорт 64 = 8 фабрик
        assert!(open_trade_order(&mut w, "GER", Steel, "SWE", 9).is_err());
        open_trade_order(&mut w, "GER", Steel, "SWE", 8).unwrap();
        assert_eq!(free_export(&w, "SWE", Steel), (0, 64));
        assert!(open_trade_order(&mut w, "GER", Steel, "XXX", 1).is_err(), "нет такой страны");
        assert!(open_trade_order(&mut w, "GER", Steel, "GER", 1).is_err(), "сам себе");
        assert!(open_trade_order(&mut w, "NOP", Steel, "SWE", 1).is_err(), "нет покупателя");
    }

    #[test]
    fn lost_states_cut_orders_proportionally() {
        let mut w = world(200); // экспорт 100 -> 12 фабрик
        open_trade_order(&mut w, "GER", Steel, "SWE", 8).unwrap();
        open_trade_order(&mut w, "FRA", Steel, "SWE", 4).unwrap();
        assert!(!enforce_export_quotas(&mut w), "в пределах лимита ничего не меняется");
        assert_eq!(w["SWE"].imported_factories, 12);
        // SWE теряет половину добычи: экспорт 50 -> 6 фабрик из 12: GER 8 -> 4, FRA 4 -> 2
        w.get_mut("SWE").unwrap().set_natural_resources(nat(100));
        assert!(enforce_export_quotas(&mut w));
        assert_eq!(w["GER"].trade_orders[0].factories, 4);
        assert_eq!(w["FRA"].trade_orders[0].factories, 2);
        assert!(exported_from(&w, "SWE", Steel) <= w["SWE"].exportable_amount(Steel));
        assert_eq!(w["SWE"].imported_factories, 6, "продавец получает меньше фабрик");
        assert_eq!(w["GER"].available_civilian_factories(), 100 - 20 - 4, "лишние фабрики вернулись покупателю");
        // потеря всего: заказы удалены
        w.get_mut("SWE").unwrap().set_natural_resources(BTreeMap::new());
        assert!(enforce_export_quotas(&mut w));
        assert!(w["GER"].trade_orders.is_empty() && w["FRA"].trade_orders.is_empty());
        assert_eq!(w["SWE"].imported_factories, 0);
    }

    #[test]
    fn midnight_tick_trims_orders_after_the_seller_loses_resources() {
        let mut w = world(200);
        open_trade_order(&mut w, "GER", Steel, "SWE", 10).unwrap(); // 80 из 100
        w.get_mut("SWE").unwrap().natural_resources = nat(80); // экспорт 40 = 5 фабрик
        let mut app = App::new();
        app.add_event::<DailyTickEvent>().insert_resource(Economies(w)).add_systems(Update, daily_production);
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
        let eco = &app.world().resource::<Economies>().0;
        assert_eq!(eco["GER"].trade_orders[0].factories, 5);
        assert_eq!(eco["SWE"].imported_factories, 5);
    }

    #[test]
    fn system_rejects_orders_over_quota_without_spending_factories() {
        let mut app = App::new();
        app.add_event::<CreateTradeOrder>()
            .add_event::<CancelTradeOrder>()
            .insert_resource(Economies(world(100)))
            .add_systems(Update, apply_trade_orders);
        for factories in [6, 1, 7] {
            app.world_mut().send_event(CreateTradeOrder {
                country: "GER".into(),
                resource: Steel,
                seller: "SWE".into(),
                factories,
            });
            app.update();
        }
        let ger = &app.world().resource::<Economies>().0["GER"];
        assert_eq!(ger.trade_orders.len(), 1, "6 (48 ед.) принят; +1 (56 > 50) и +7 отклонены");
        assert_eq!(ger.dedicated_trade_factories(), 6);
    }

    #[test]
    fn natural_resources_follow_control_of_states() {
        let state = |id: u32, owner: &str, provinces: Vec<u32>, steel: u32| {
            Arc::new(StateDef {
                id,
                name: format!("S{id}"),
                owner: owner.into(),
                provinces,
                victory_points: vec![],
                civilian_factories: 0,
                military_factories: 0,
                resources: nat(steel),
            })
        };
        let registry = StateRegistry {
            states: vec![state(1, "SWE", vec![1, 2, 3], 50), state(2, "SWE", vec![4, 5], 30)],
            province_to_state: Default::default(),
        };
        let mut controllers = ProvinceControllers(
            [(1, "SWE"), (2, "SWE"), (3, "SWE"), (4, "SWE"), (5, "SWE")].iter().map(|&(p, c)| (p, c.to_string())).collect(),
        );
        let start = natural_by_controller(&registry, &controllers);
        assert_eq!(start["SWE"][&Steel], 80);
        // GER занимает 2 из 3 провинций первого стейта: ресурсы стейта переходят к GER
        controllers.0.insert(1, "GER".into());
        controllers.0.insert(2, "GER".into());
        let after = natural_by_controller(&registry, &controllers);
        assert_eq!((after["GER"][&Steel], after["SWE"][&Steel]), (50, 30));
        // ровно половина второго стейта (1 из 2) — большинства нет, остаётся владелец
        controllers.0.insert(4, "GER".into());
        assert_eq!(natural_by_controller(&registry, &controllers)["SWE"][&Steel], 30);
    }

    #[test]
    fn export_panel_text_numbers() {
        let mut w = world(100);
        open_trade_order(&mut w, "GER", Steel, "SWE", 2).unwrap();
        let (free, total) = free_export(&w, "SWE", Steel);
        assert_eq!(format!("Доступно для импорта: {free}/{total} ед."), "Доступно для импорта: 34/50 ед.");
    }

    /// Реальные данные: добыча SWE читается из стейтов, а стартовый заказ GER на 24 стали проходит квоту.
    #[test]
    fn real_data_start_contract_passes_sweden_quota() {
        let root = std::path::Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let registry = crate::map::load_all_states(&root.join("history").join("states")).expect("стейты");
        let mut natural = registry.resources_by_owner();
        let mut economies = BTreeMap::new();
        for (tag, (civ, mil)) in registry.factories_by_owner() {
            let mut eco = CountryEconomy::new(civ, mil);
            eco.set_natural_resources(natural.remove(&tag).unwrap_or_default());
            economies.insert(tag, eco);
        }
        let sweden = &economies["SWE"];
        println!("SWE: добыча стали {}, экспорт {}", sweden.natural(Steel), sweden.exportable_amount(Steel));
        assert!(sweden.exportable_amount(Steel) >= 24, "экспорт стали Швеции покрывает 3 фабрики");
        assert!(economies["GER"].natural(Steel) > 0);
        open_trade_order(&mut economies, "GER", Steel, "SWE", START_TRADE_FACTORIES).expect("стартовый контракт");
        assert_eq!(economies["GER"].imported_resources(Steel), 24);
    }
}
