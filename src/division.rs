//! Дивизии: спавн, выбор (ЛКМ), приказы на марш по графу провинций (ПКМ),
//! движение и бой по часовым тикам `GameClock`.
//!
//! Ввод только формирует событие `OrderMove`; логика (`movement`, `current_province`, бой)
//! меняется в `apply_orders`, `advance_marches` и `resolve_combats` целыми числами по `HourTick`.
//! Позиции, масштаб спрайтов и UI-полоски — чисто визуальные и на симуляцию не влияют.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashSet};

use bevy::prelude::*;
use bevy::sprite::Anchor;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts};

use crate::camera::MainCamera;
use crate::clock::{ClockSet, DailyTickEvent, HourTick};
use crate::air::{air_attack_bonus_pct, air_defense_penalty_pct, air_line, StrategicAirRegions};
use crate::colors::CountryColors;
use crate::command::{
    resolve_counters, side_mods, CombatPhase, CombatTactic, ATTACKER_TACTICS, DEFENDER_TACTICS,
    SKILL_BONUS_PCT, WITHDRAWAL_ORG_DAMAGE_PCT, WITHDRAWAL_PHASE_HOURS, WITHDRAWAL_WIDTH_DIVISOR,
    pick_tactic_boosted, pick_tactic_scaled, BRIDGE_ATTACKER_TACTICS, BRIDGE_DEFENDER_TACTICS, BRIDGE_PHASE_HOURS,
    CLOSE_QUARTERS_DAMAGE_PCT, CLOSE_QUARTERS_PHASE_HOURS, CQ_ATTACKER_TACTICS, CQ_DEFENDER_TACTICS,
    URBAN_ASSAULT_WEIGHT_MULT, ATTACKER_TACTICS_WITH_BREAKTHROUGH, BREAKTHROUGH_PHASE_HOURS,
    DEFENDER_TACTICS_WITH_AMBUSH, breakthrough_available,
};
use crate::diplomacy::{check_entry, DiplomacyRegistry};
use crate::economy::{update_supply_network, ArmyExperience, EquipmentType, PlayerCountry, SupplyNetwork};
use crate::oob::{resolve_placement, resolve_template, OobDivisions, Placement};
use crate::units::{
    DivisionExperienceLevel, DivisionTemplate, DivisionTemplates, NatoSymbol, BASE_TEMPLATE, INFANTRY_SPEED_KPH,
    MAX_EXPERIENCE_PCT,
};
#[cfg(test)]
use crate::units::PLAIN_INFANTRY_TEMPLATE;
use crate::map::{
    pick_province, setup_map, HoveredProvince, ProvinceAdjacency, ProvinceCenters, ProvinceControllerChanged,
    ProvinceControllers, ProvinceMap, ProvinceRivers, StateRegistry,
};

/// Часов боя на линии на каждый процент опыта дивизии (сутки на линии дают +4%).
pub const XP_HOURS_PER_PCT: u32 = 6;

/// Интенсивность боя: доля организации цели, снимаемая за час, в сотых долях процента (bp),
/// при равных атаке и защите. Подобрана так, чтобы равные дивизии дрались около суток.
const COMBAT_INTENSITY_BP: u32 = 800;
/// Максимум планирования: +30% к атаке наступающей дивизии.
pub const MAX_PLANNING: u32 = 30;
/// Небоевые потери отрезанной дивизии: доля текущей прочности в сутки, %.
const ATTRITION_STRENGTH_PCT: u32 = 2;
/// Отрезанная от снабжения дивизия сохраняет столько процентов эффективной атаки и защиты (-33%).
const UNSUPPLIED_STAT_PCT: u32 = 67;
/// Отдых: снабжённая дивизия вне боя и марша восстанавливает столько % максимальной организации в сутки.
const ORG_REGEN_PCT_PER_DAY: u32 = 10;
/// Прирост планирования в сутки для дивизии на рубеже фронта.
pub const PLANNING_GAIN_PER_DAY: u32 = 2;
/// Расход планирования в сутки на марше и в бою.
pub const PLANNING_DECAY_PER_DAY: u32 = 3;
/// Базовая ширина фронта столкновения (на каждую из сторон).
const BASE_COMBAT_WIDTH: u32 = 80;
/// Прибавка к ширине за каждое дополнительное направление атаки (удар из другой соседней провинции).
const FLANK_WIDTH_BONUS: u32 = 40;
/// Минимальный шанс (%) выхода резерва на линию за час; к нему добавляется 0..=3 по id дивизии (итого 2..=5%).
const RESERVE_MIN_CHANCE_PCT: u32 = 2;

const DIVISION_SIZE: Vec2 = Vec2::new(26.0, 18.0);
/// Ширина полоски организации / прочности на фишке при 100%, пиксели.
/// Микро-полоски под фишкой: ширина равна ширине фишки, высота каждой 3 px.
const BAR_MAX_WIDTH: f32 = 26.0;
const BAR_HEIGHT: f32 = 3.0;
const ORG_BAR_COLOR: Color = Color::srgb(46.0 / 255.0, 139.0 / 255.0, 87.0 / 255.0);
const STRENGTH_BAR_COLOR: Color = Color::srgb(198.0 / 255.0, 120.0 / 255.0, 20.0 / 255.0);
const FRAME_COLOR: Color = Color::srgb(0.9, 0.9, 0.86);
const SELECTED_FRAME_COLOR: Color = Color::srgb(1.0, 0.85, 0.1);
const BAR_BACKDROP_COLOR: Color = Color::srgb(0.2, 0.2, 0.22);
const DIVISION_Z: f32 = 10.0;
/// Скорость сглаживания визуальной позиции (1/сек), только для отрисовки.
const VISUAL_SMOOTHING: f32 = 10.0;
/// Максимальный сдвиг курсора между нажатием и отпусканием ПКМ, чтобы считать это кликом, а не панорамой.
const CLICK_MAX_DRAG: f32 = 6.0;

// ---------------------------------------------------------------------------
// Компоненты, ресурсы, события
// ---------------------------------------------------------------------------

/// Состояние марша. Все поля целочисленные.
/// `hours_left == 0` — дивизия дошла до границы и ждёт: либо занимает провинцию, либо вступает в бой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovementProgress {
    pub from: u32,
    pub to: u32,
    pub hours_left: u32,
    pub total_hours: u32,
}

#[derive(Component, Debug, Clone)]
pub struct Division {
    pub id: u32,
    /// Историческое имя из OOB.
    pub name: String,
    pub owner: String,
    /// Провинция, где дивизия находится; во время марша — провинция отправления.
    pub current_province: u32,
    pub movement: Option<MovementProgress>,
    /// Очередь маршрута: следующие провинции после текущего шага (без текущей).
    pub path: Vec<u32>,
    pub max_org: u32,
    pub current_org: u32,
    pub max_strength: u32,
    pub current_strength: u32,
    pub soft_attack: u32,
    pub defense: u32,
    /// Накопленные безвозвратные боевые потери (единиц прочности ~ винтовок).
    pub losses: u32,
    /// Имя шаблона из OOB (отображается в панели).
    pub template_name: String,
    pub battalion_count: u32,
    /// Личный состав и снаряжение в полном составе (из шаблона).
    pub max_manpower: u32,
    /// Потребность в снаряжении по типам для полного состава (из шаблона).
    pub equipment_needs: BTreeMap<EquipmentType, u32>,
    /// Типы снаряжения, которых не хватило при последнем суточном пополнении (дефицит сдерживает рост прочности).
    pub equipment_shortage: BTreeSet<EquipmentType>,
    /// Символ NATO на фишке: определяется шаблоном.
    pub nato_symbol: NatoSymbol,
    /// Опыт дивизии, % (0..=100): стартует из OOB (`start_experience_factor`), растёт в бою, размывается
    /// пополнениями. Определяет ранг и множитель боевой мощи.
    pub experience_pct: u32,
    /// Часы боя на линии, ещё не превращённые в процент опыта (меньше `XP_HOURS_PER_PCT`).
    pub combat_xp_hours: u32,
    /// Стартовая оснащённость из OOB (`start_equipment_factor`), %; справочное значение.
    pub start_equipment_pct: u32,
    /// Ещё не списанный дробный урон организации, в тысячных долях очка (0..1000).
    pub org_frac: u32,
    /// Окопанность 0..=10: растёт раз в сутки на месте, сбрасывается при марше и отступлении.
    pub entrenchment: u32,
    /// Ширина фронта, которую дивизия занимает на активной линии боя (из шаблона).
    pub combat_width: u32,
    /// Роль в текущем бою: вне боя, на активной линии или в резерве.
    pub combat_state: CombatState,
    /// Скорость дивизии (км/ч × 10) — по самому медленному батальону.
    pub max_speed: u32,
    pub armor: u32,
    pub piercing: u32,
    /// Твердость, %.
    pub hardness: u32,
    /// Разведка: максимум среди батальонов. Сторона с лучшей разведкой в бою получает тактическое преимущество.
    pub recon: u32,
    /// Жёсткая атака (сумма по батальонам и ротам обеспечения); пока только показывается в интерфейсе.
    pub hard_attack: u32,
    /// Предел окопанности: 10, с сапёрами 15. Каждый пункт даёт +2% к защите.
    pub max_entrenchment: u32,
    /// На сколько пунктов сапёры снижают штраф форсирования реки (0 или 20).
    pub river_penalty_reduction: u32,
    /// Роты обеспечения для интерфейса: «Сапёры», «Арт», «Разведка».
    pub support: Vec<&'static str>,
    /// Планирование 0..=`MAX_PLANNING` (%): копится на рубеже фронта, тратится на марше и в бою.
    pub planning: u32,
    /// Навыки командира фронта, на который назначена дивизия (+2% за уровень), иначе 0.
    /// Заполняются `command::update_command_effects`.
    pub command_attack_skill: u32,
    pub command_defense_skill: u32,
    /// Дивизия сверх лимита командира: -50% к получаемому планированию и к боевым параметрам.
    pub over_command_limit: bool,
    /// Снабжается ли дивизия хабом своей нации (проверка раз в сутки).
    pub is_supplied: bool,
    /// Сколько суток подряд дивизия отрезана от снабжения.
    pub attrition_days: u32,
}

/// Положение дивизии в бою. Резерв не наносит и не получает урон.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CombatState {
    #[default]
    None,
    Active,
    Reserve,
}

impl Division {
    /// Личный состав сейчас: пропорционален укомплектованности.
    pub fn current_manpower(&self) -> u32 {
        (u64::from(self.max_manpower) * u64::from(self.current_strength) / u64::from(self.max_strength.max(1))) as u32
    }

    /// Нужно единиц снаряжения всех типов для полного состава.
    pub fn required_equipment(&self) -> u32 {
        self.equipment_needs.values().sum()
    }

    /// Снаряжение по типу сейчас: потребность типа, умноженная на укомплектованность дивизии.
    pub fn equipment_of(&self, equipment: EquipmentType) -> u32 {
        let need = self.equipment_needs.get(&equipment).copied().unwrap_or(0);
        (u64::from(need) * u64::from(self.current_strength) / u64::from(self.max_strength.max(1))) as u32
    }

    /// Снаряжение всех типов сейчас.
    pub fn current_equipment(&self) -> u32 {
        self.equipment_needs.keys().map(|&t| self.equipment_of(t)).sum()
    }
    /// Урон организации в тысячных долях очка; дробный остаток копится в `org_frac`.
    fn lose_org_milli(&mut self, milli: u32) {
        let total = self.org_frac.saturating_add(milli);
        self.current_org = self.current_org.saturating_sub(total / 1000);
        self.org_frac = if self.current_org == 0 { 0 } else { total % 1000 };
    }

    /// Организация в тысячных долях очка (с учётом ещё не списанного урона).
    fn org_milli(&self) -> u32 {
        (self.current_org * 1000).saturating_sub(self.org_frac)
    }

    /// Боевые потери: уменьшает прочность и фиксирует реально потерянное в `losses`.
    fn lose_strength(&mut self, amount: u32) {
        let lost = amount.min(self.current_strength);
        self.current_strength -= lost;
        self.losses = self.losses.saturating_add(lost);
    }

    /// Переформирование по новому шаблону: характеристики берутся из `calculate_stats()`,
    /// окопанность обнуляется, текущая организация делится пополам (и не выше нового максимума),
    /// а прочность сохраняет свою долю от максимума. Накопленные потери, планирование и снабжение остаются.
    pub fn refit(&mut self, template: &DivisionTemplate) {
        let stats = template.calculate_stats();
        let old_max = u64::from(self.max_strength.max(1));
        let scaled = (u64::from(self.current_strength) * u64::from(stats.max_strength) / old_max) as u32;
        self.current_strength = if self.current_strength == 0 { 0 } else { scaled.max(1).min(stats.max_strength) };
        self.current_org = (self.current_org / 2).min(stats.max_org);
        self.org_frac = 0;
        self.entrenchment = 0;
        self.path.clear();

        self.template_name = template.name.clone();
        self.battalion_count = template.battalion_count();
        self.max_org = stats.max_org;
        self.max_strength = stats.max_strength;
        self.max_manpower = stats.total_manpower;
        self.equipment_needs = stats.equipment_needs;
        self.equipment_shortage.clear();
        self.soft_attack = stats.soft_attack;
        self.hard_attack = stats.hard_attack;
        self.defense = stats.defense;
        self.combat_width = stats.combat_width;
        self.max_speed = stats.max_speed;
        self.armor = stats.armor;
        self.piercing = stats.piercing;
        self.hardness = stats.hardness;
        self.recon = stats.recon;
        self.max_entrenchment = stats.max_entrenchment;
        self.river_penalty_reduction = stats.river_penalty_reduction;
        self.support = stats.support;
        self.nato_symbol = stats.nato_symbol;
    }

    /// Дивизия в полном составе; все характеристики считает `DivisionTemplate::calculate_stats()`.
    pub fn from_template(
        id: u32,
        name: String,
        owner: String,
        province: u32,
        template_name: String,
        template: &DivisionTemplate,
    ) -> Self {
        let stats = template.calculate_stats();
        Self {
            id,
            name,
            owner,
            current_province: province,
            movement: None,
            path: Vec::new(),
            max_org: stats.max_org,
            current_org: stats.max_org,
            max_strength: stats.max_strength,
            current_strength: stats.max_strength,
            soft_attack: stats.soft_attack,
            defense: stats.defense,
            losses: 0,
            template_name,
            battalion_count: template.battalion_count(),
            max_manpower: stats.total_manpower,
            equipment_needs: stats.equipment_needs,
            equipment_shortage: BTreeSet::new(),
            nato_symbol: stats.nato_symbol,
            experience_pct: 0,
            combat_xp_hours: 0,
            start_equipment_pct: 100,
            org_frac: 0,
            entrenchment: 0,
            combat_width: stats.combat_width,
            combat_state: CombatState::None,
            max_speed: stats.max_speed,
            armor: stats.armor,
            piercing: stats.piercing,
            hardness: stats.hardness,
            recon: stats.recon,
            hard_attack: stats.hard_attack,
            max_entrenchment: stats.max_entrenchment,
            river_penalty_reduction: stats.river_penalty_reduction,
            support: stats.support,
            planning: 0,
            command_attack_skill: 0,
            command_defense_skill: 0,
            over_command_limit: false,
            is_supplied: true,
            attrition_days: 0,
        }
    }
}

/// Идущий бой за провинцию. Отдельная сущность, существует до победы одной из сторон.
/// В бою участвуют все дивизии обеих сторон; на линию каждой стороны попадает столько,
/// сколько вмещает `max_width`, остальные ждут в резерве (`Division::combat_state`).
#[derive(Component, Debug, Clone)]
pub struct ActiveCombat {
    pub province_id: u32,
    pub attacker_owner: String,
    pub defender_owner: String,
    /// Все атакующие (линия и резерв), по возрастанию id дивизии.
    pub attackers: Vec<Entity>,
    /// Все защитники (линия и резерв), по возрастанию id дивизии.
    pub defenders: Vec<Entity>,
    /// Лимит ширины фронта для каждой стороны: база + бонус за каждое дополнительное направление атаки.
    pub max_width: u32,
    /// Процент потерянной защитниками организации, 0..=100.
    pub combat_progress: u32,
    /// Сколько игровых часов длится бой (детерминированный источник «случайности» для резервов и отступления).
    pub elapsed_hours: u32,
    /// Тактика наступающей стороны (до первого суточного выбора — базовая атака).
    pub attacker_tactic: CombatTactic,
    /// Тактика обороняющейся стороны (до первого суточного выбора — базовая оборона).
    pub defender_tactic: CombatTactic,
    /// Тактика атакующих парирована: бонусы своей тактики потеряны.
    pub is_attacker_countered: bool,
    /// Тактика защитников парирована.
    pub is_defender_countered: bool,
    /// Индекс игровых суток последнего выбора тактик.
    pub tactic_change_day: u32,
    /// Фаза сражения: отход защитников сужает фронт и смягчает урон.
    pub phase: CombatPhase,
    /// Сколько игровых часов фаза ещё длится (уменьшается каждый `HourTick`).
    pub phase_hours_left: u32,
    /// Мост за атакующими: речной штраф форсирования снят (держится и после выхода из речной фазы).
    pub bridge_held: bool,
}

impl ActiveCombat {
    /// Участвует ли дивизия в бою (на линии или в резерве).
    pub fn involves(&self, entity: Entity) -> bool {
        self.attackers.contains(&entity) || self.defenders.contains(&entity)
    }

    /// Лимит ширины линии каждой стороны с учётом фазы: при отходе вдвое меньше.
    pub fn effective_max_width(&self) -> u32 {
        match self.phase {
            CombatPhase::Default | CombatPhase::Bridge | CombatPhase::Breakthrough => self.max_width,
            CombatPhase::TacticalWithdrawal | CombatPhase::CloseQuarters => self.max_width / WITHDRAWAL_WIDTH_DIVISOR,
        }
    }
}

#[derive(Resource, Default)]
pub struct SelectedDivision(pub Option<Entity>);

impl Division {
    /// Тест: опыт 10% (Trained, множитель ровно 100%) — чтобы формулы других модификаторов проверялись отдельно.
    #[cfg(test)]
    pub(crate) fn trained(mut self) -> Self {
        self.experience_pct = 10;
        self
    }

    /// Ранг опыта дивизии.
    pub fn experience_level(&self) -> DivisionExperienceLevel {
        DivisionExperienceLevel::from_experience_pct(self.experience_pct)
    }

    /// Множитель боевой мощи по опыту, %: 75 / 100 / 125 / 150 / 175 (всегда положительный).
    pub fn experience_multiplier_pct(&self) -> u32 {
        self.experience_level().combat_multiplier_pct().max(0) as u32
    }

    /// Час боя на линии: каждые `XP_HOURS_PER_PCT` часов дают +1% опыта, потолок 100%.
    pub fn gain_combat_experience_hour(&mut self) {
        if self.experience_pct >= MAX_EXPERIENCE_PCT {
            self.experience_pct = MAX_EXPERIENCE_PCT;
            self.combat_xp_hours = 0;
            return;
        }
        self.combat_xp_hours += 1;
        if self.combat_xp_hours >= XP_HOURS_PER_PCT {
            self.combat_xp_hours -= XP_HOURS_PER_PCT;
            self.experience_pct += 1;
        }
    }

    /// Размытие опыта пополнением: новобранцы приходят с 0%, поэтому
    /// `новый = текущая_прочность * опыт / (текущая_прочность + пополнение)` (целочисленно, вниз).
    /// Вызывается до увеличения прочности.
    pub fn dilute_experience(&mut self, reinforce_amount: u32) {
        let total = u64::from(self.current_strength) + u64::from(reinforce_amount);
        if total == 0 || reinforce_amount == 0 {
            return;
        }
        self.experience_pct = (u64::from(self.current_strength) * u64::from(self.experience_pct) / total) as u32;
    }
}

/// Строка опыта для окна «Дивизия»: `Опыт: 30% (Кадровые, +25% к бою)`.
pub fn experience_text(experience_pct: u32) -> String {
    let level = DivisionExperienceLevel::from_experience_pct(experience_pct);
    let bonus = level.combat_multiplier_pct() - 100;
    let sign = if bonus > 0 { "+" } else { "" };
    format!("Опыт: {experience_pct}% ({}, {sign}{bonus}% к бою)", level.title())
}

/// Режим выбора: `whole` — выбран весь стек провинции (ПКМ командует всеми стоящими в ней дивизиями
/// того же владельца), иначе — только `SelectedDivision` (выбор строки в инспекторе).
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedGroup {
    pub whole: bool,
}

/// Протяжка рамки выделения. Экранные координаты в логических пикселях окна — только клиентский ввод.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct DragSelection {
    /// ЛКМ зажата, рамка тянется.
    pub active: bool,
    /// Точка нажатия ЛКМ.
    pub start_screen_pos: Vec2,
    /// Текущее положение курсора.
    pub current_screen_pos: Vec2,
    /// Минимальная протяжка в пикселях, отличающая рамку от клика.
    pub threshold: f32,
}

impl Default for DragSelection {
    fn default() -> Self {
        Self { active: false, start_screen_pos: Vec2::ZERO, current_screen_pos: Vec2::ZERO, threshold: 8.0 }
    }
}

impl DragSelection {
    /// Протяжка длиннее порога: это рамка, а не клик.
    pub fn is_box(&self) -> bool {
        self.start_screen_pos.distance(self.current_screen_pos) > self.threshold
    }
}

/// Множественный выбор рамкой: дивизии игрока по возрастанию id. Пусто — действует обычный выбор стека.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct SelectedDivisions(pub Vec<Entity>);

/// Прямоугольник из двух углов в любом порядке: (минимум, максимум).
pub fn normalized_rect(a: Vec2, b: Vec2) -> (Vec2, Vec2) {
    (a.min(b), a.max(b))
}

/// Точка внутри AABB (границы включены).
pub fn rect_contains(min: Vec2, max: Vec2, point: Vec2) -> bool {
    point.x >= min.x && point.x <= max.x && point.y >= min.y && point.y <= max.y
}

/// Дивизии владельца `owner`, центры которых лежат в прямоугольнике; элементы — `(сущность, id, владелец, центр)`.
/// Результат по возрастанию id (детерминированный порядок).
pub fn boxed_divisions<'a>(
    items: impl IntoIterator<Item = (Entity, u32, &'a str, Vec2)>,
    owner: &str,
    min: Vec2,
    max: Vec2,
) -> Vec<Entity> {
    let mut found: Vec<(u32, Entity)> = items
        .into_iter()
        .filter(|&(_, _, o, center)| o == owner && rect_contains(min, max, center))
        .map(|(e, id, _, _)| (id, e))
        .collect();
    found.sort_unstable();
    found.into_iter().map(|(_, e)| e).collect()
}

/// Штрих символа NATO на фишке (дочерний спрайт); пересоздаётся при смене шаблона.
#[derive(Component)]
struct NatoStroke;

/// Какой символ сейчас нарисован на фишке (для перерисовки после переформирования).
#[derive(Component)]
struct NatoIcon(NatoSymbol);

/// Отрезок символа в половинках пикселя относительно центра фишки: `(x1, y1, x2, y2)`; нулевая длина — точка.
pub type StrokeHp = (i32, i32, i32, i32);

/// cos и sin углов k * 30 градусов в тысячных долях (целые, без f32): вершины ломаной-эллипса.
const TRIG_30: [(i32, i32); 12] = [
    (1000, 0), (866, 500), (500, 866), (0, 1000), (-500, 866), (-866, 500),
    (-1000, 0), (-866, -500), (-500, -866), (0, -1000), (500, -866), (866, -500),
];

/// Центр области символа над полосками, половинки пикселя.
const SYMBOL_CENTER_HP: (i32, i32) = (0, 5);

/// Штрихи символа NATO (целые половинки пикселя): X — две диагонали, кавалерия — одна, мотопехота — X и два
/// колеса, бронетехника — 12-угольник-эллипс, артиллерия — точка.
pub fn symbol_strokes(symbol: NatoSymbol) -> Vec<StrokeHp> {
    let (cx, cy) = SYMBOL_CENTER_HP;
    match symbol {
        NatoSymbol::Infantry => vec![(cx - 18, cy - 8, cx + 18, cy + 8), (cx - 18, cy + 8, cx + 18, cy - 8)],
        NatoSymbol::Cavalry => vec![(cx - 18, cy - 8, cx + 18, cy + 8)],
        NatoSymbol::Motorized => vec![
            (cx - 18, cy - 4, cx + 18, cy + 10),
            (cx - 18, cy + 10, cx + 18, cy - 4),
            (cx - 8, cy - 7, cx - 8, cy - 7),
            (cx + 8, cy - 7, cx + 8, cy - 7),
        ],
        NatoSymbol::Armored => {
            let (rx, ry) = (18, 8);
            (0..12)
                .map(|k| {
                    let (c1, s1) = TRIG_30[k];
                    let (c2, s2) = TRIG_30[(k + 1) % 12];
                    (cx + rx * c1 / 1000, cy + ry * s1 / 1000, cx + rx * c2 / 1000, cy + ry * s2 / 1000)
                })
                .collect()
        }
        NatoSymbol::Artillery => vec![(cx, cy, cx, cy)],
    }
}

/// Цвет линий символа: чёрный на светлой нации, белый на тёмной (яркость в целых: R*299 + G*587 + B*114).
pub fn symbol_line_color(nation_rgb: [u8; 3]) -> Color {
    let luma = (u32::from(nation_rgb[0]) * 299 + u32::from(nation_rgb[1]) * 587 + u32::from(nation_rgb[2]) * 114) / 1000;
    if luma > 128 { Color::srgb(0.02, 0.02, 0.02) } else { Color::srgb(0.98, 0.98, 0.98) }
}

/// Спавнит штрихи символа как дочерние спрайты фишки; масштабируются вместе с ней (billboard).
fn spawn_nato_strokes(parent: &mut ChildBuilder, symbol: NatoSymbol, color: Color) {
    for (x1, y1, x2, y2) in symbol_strokes(symbol) {
        let (a, b) = (Vec2::new(x1 as f32 / 2.0, y1 as f32 / 2.0), Vec2::new(x2 as f32 / 2.0, y2 as f32 / 2.0));
        let delta = b - a;
        let length = delta.length();
        // точка (нулевая длина) — квадрат 3x3, отрезок — тонкий прямоугольник вдоль линии
        let (size, angle) = if length < 0.01 { (Vec2::splat(3.0), 0.0) } else { (Vec2::new(length + 1.0, 1.0), delta.y.atan2(delta.x)) };
        parent.spawn((
            SpriteBundle {
                sprite: Sprite { color, custom_size: Some(size), ..default() },
                transform: Transform {
                    translation: ((a + b) * 0.5).extend(0.25),
                    rotation: Quat::from_rotation_z(angle),
                    ..default()
                },
                ..default()
            },
            NatoStroke,
        ));
    }
}

/// Перерисовывает значок, если шаблон дивизии (а с ним символ) сменился: старые штрихи удаляются.
fn update_nato_icons(
    mut commands: Commands,
    colors: Res<CountryColors>,
    mut divisions: Query<(Entity, &Division, &mut NatoIcon, Option<&Children>)>,
    strokes: Query<(), With<NatoStroke>>,
) {
    for (entity, d, mut icon, children) in &mut divisions {
        if icon.0 == d.nato_symbol {
            continue;
        }
        for &child in children.into_iter().flatten() {
            if strokes.get(child).is_ok() {
                commands.entity(child).despawn();
            }
        }
        let color = symbol_line_color(colors.map_rgb(&d.owner));
        commands.entity(entity).with_children(|p| spawn_nato_strokes(p, d.nato_symbol, color));
        icon.0 = d.nato_symbol;
    }
}

/// Полоска организации на фишке (дочерний спрайт).
#[derive(Component)]
struct OrgBar;
/// Полоска прочности на фишке (дочерний спрайт).
#[derive(Component)]
struct StrengthBar;

/// Стек стоящих дивизий одной провинции: главная фишка и члены по возрастанию id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackInfo {
    pub head: Entity,
    pub members: Vec<(u32, Entity)>,
}

/// Текущий вид стеков по провинциям (обновляется каждый кадр; нужен значкам и тестам).
#[derive(Resource, Default)]
pub struct StackView(pub BTreeMap<u32, StackInfo>);

/// Собирает стеки стоящих дивизий: `(сущность, id дивизии, провинция)`. Главная фишка — выбранная дивизия
/// стека, иначе с наименьшим id. Порядок членов и результат детерминированы.
pub fn stack_layout(items: impl IntoIterator<Item = (Entity, u32, u32)>, selected: Option<Entity>) -> BTreeMap<u32, StackInfo> {
    let mut grouped: BTreeMap<u32, Vec<(u32, Entity)>> = BTreeMap::new();
    for (entity, id, province) in items {
        grouped.entry(province).or_default().push((id, entity));
    }
    grouped
        .into_iter()
        .map(|(province, mut members)| {
            members.sort_unstable_by_key(|&(id, _)| id);
            let head = selected
                .filter(|sel| members.iter().any(|&(_, e)| e == *sel))
                .unwrap_or(members[0].1);
            (province, StackInfo { head, members })
        })
        .collect()
}

/// Процент `cur / max` целыми числами (0 при нулевом максимуме), не больше 100.
pub fn percent(cur: u32, max: u32) -> u32 {
    if max == 0 {
        return 0;
    }
    (u64::from(cur) * 100 / u64::from(max)).min(100) as u32
}

/// Среднее арифметическое процентов (целое, вниз); пустой набор — 0.
pub fn average_percent(values: &[u32]) -> u32 {
    if values.is_empty() {
        return 0;
    }
    (values.iter().map(|&v| u64::from(v)).sum::<u64>() / values.len() as u64) as u32
}

/// Бейдж типа дивизии в инспекторе: `[NATO: Пехота]`.
pub fn nato_badge_text(symbol: NatoSymbol) -> String {
    format!("[NATO: {}]", symbol.label())
}

/// Значок количества дивизий в стеке в квадратной рамке: `[3]`.
pub fn stack_badge_text(count: usize) -> String {
    format!("[{count}]")
}

/// Строка инспектора: `1. Infanterie-Division | Орг: 100% | HP: 100%`.
pub fn stack_row_text(index: usize, d: &Division) -> String {
    format!(
        "{index}. {} | Орг: {}% | HP: {}%",
        d.name,
        percent(d.current_org, d.max_org),
        percent(d.current_strength, d.max_strength)
    )
}

/// Адресаты приказа ПКМ с учётом рамки: при выделении рамкой — все живые дивизии группы по порядку,
/// иначе обычная логика стека / одной дивизии.
pub fn command_targets(
    multi: &SelectedDivisions,
    divisions: &Query<(Entity, &Division)>,
    selected: Entity,
    whole: bool,
) -> Vec<Entity> {
    if multi.0.is_empty() {
        return order_targets(divisions, selected, whole);
    }
    multi.0.iter().copied().filter(|&e| divisions.get(e).is_ok()).collect()
}

/// Кому отдаётся приказ ПКМ: вся группа (стоящие дивизии провинции выбранной, того же владельца, по id)
/// либо только выбранная дивизия.
pub fn order_targets(divisions: &Query<(Entity, &Division)>, selected: Entity, whole: bool) -> Vec<Entity> {
    let Ok((_, sel)) = divisions.get(selected) else { return Vec::new() };
    if !whole || sel.movement.is_some() {
        return vec![selected];
    }
    let mut group: Vec<(u32, Entity)> = divisions
        .iter()
        .filter(|(_, d)| d.current_province == sel.current_province && d.movement.is_none() && d.owner == sel.owner)
        .map(|(e, d)| (d.id, e))
        .collect();
    group.sort_unstable();
    group.into_iter().map(|(_, e)| e).collect()
}

/// Приказ на перемещение дивизии в соседнюю провинцию.
#[derive(Event, Debug, Clone, Copy)]
pub struct OrderMove {
    pub division: Entity,
    pub target: u32,
}

/// Команда: переформировать дивизию по другому шаблону (Refit). Выдаётся из интерфейса,
/// применяется в pply_refits.
#[derive(Event, Debug, Clone)]
pub struct RefitDivision {
    pub division: Entity,
    pub template: String,
}

/// Применяет команды `RefitDivision`. Шаблон должен быть доступен нации дивизии, а сама дивизия —
/// стоять на месте и не участвовать в бою: смена ширины посреди боя нарушила бы его учёт.
fn apply_refits(
    mut events: EventReader<RefitDivision>,
    templates: Res<DivisionTemplates>,
    mut divisions: Query<&mut Division>,
    combats: Query<&ActiveCombat>,
) {
    for event in events.read() {
        let Ok(mut d) = divisions.get_mut(event.division) else { continue };
        let Some(template) = templates.get(&event.template) else {
            println!("[ПЕРЕФОРМИРОВАНИЕ] Шаблон \"{}\" не найден", event.template);
            continue;
        };
        if !templates.is_available(&d.owner, &event.template) {
            println!("[ПЕРЕФОРМИРОВАНИЕ] Шаблон \"{}\" недоступен для {}", event.template, d.owner);
            continue;
        }
        if d.movement.is_some() || combats.iter().any(|c| c.involves(event.division)) {
            println!("[ПЕРЕФОРМИРОВАНИЕ] Дивизия #{} в бою или на марше — шаблон не сменён", d.id);
            continue;
        }
        d.refit(template);
        println!("[ПЕРЕФОРМИРОВАНИЕ] Дивизия #{} переформирована в \"{}\"", d.id, template.name);
    }
}

/// Раз в сутки (00:00), строго после пересчёта сети снабжения (`update_supply_network`):
/// - дивизия в зоне покрытия хаба своей нации снабжена, счётчик истощения обнуляется; стоящая вне боя
///   снабжённая дивизия отдыхает и восстанавливает организацию;
/// - отрезанная дивизия: `is_supplied = false`, +1 день истощения, организация не восстанавливается,
///   теряется `ATTRITION_STRENGTH_PCT`% текущей прочности (не ниже 1 единицы, минимум потери 1).
///   Эти небоевые потери не входят в счётчик боевых `losses`.
pub fn daily_supply(
    mut events: EventReader<DailyTickEvent>,
    network: Res<SupplyNetwork>,
    mut divisions: Query<(Entity, &mut Division)>,
    combats: Query<&ActiveCombat>,
) {
    for _ in events.read() {
        let in_combat: HashSet<Entity> =
            combats.iter().flat_map(|c| c.attackers.iter().chain(c.defenders.iter()).copied()).collect();
        let (mut cut_off, mut total) = (0u32, 0u32);
        for (entity, mut d) in &mut divisions {
            total += 1;
            if network.supplies(d.current_province, &d.owner) {
                if !d.is_supplied || d.attrition_days != 0 {
                    d.is_supplied = true;
                    d.attrition_days = 0;
                }
                if d.movement.is_none() && !in_combat.contains(&entity) && d.current_org < d.max_org {
                    let regen = (d.max_org * ORG_REGEN_PCT_PER_DAY / 100).max(1);
                    d.current_org = (d.current_org + regen).min(d.max_org);
                }
            } else {
                cut_off += 1;
                d.is_supplied = false;
                d.attrition_days += 1;
                let loss = (d.current_strength * ATTRITION_STRENGTH_PCT / 100)
                    .max(1)
                    .min(d.current_strength.saturating_sub(1));
                d.current_strength -= loss;
            }
        }
        if cut_off > 0 {
            println!("[СНАБЖЕНИЕ] Отрезано от снабжения дивизий: {cut_off} из {total}");
        }
    }
}

/// Раз в сутки (00:00): стоящая дивизия, не в бою и без приказа на марш, окапывается на 1 пункт.
fn daily_entrenchment(
    mut events: EventReader<DailyTickEvent>,
    mut divisions: Query<(Entity, &mut Division)>,
    combats: Query<&ActiveCombat>,
) {
    for _ in events.read() {
        let in_combat: HashSet<Entity> =
            combats.iter().flat_map(|c| c.attackers.iter().chain(c.defenders.iter()).copied()).collect();
        for (entity, mut d) in &mut divisions {
            if d.movement.is_none()
                && d.path.is_empty()
                && !in_combat.contains(&entity)
                && d.entrenchment < d.max_entrenchment
            {
                d.entrenchment += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Плагин
// ---------------------------------------------------------------------------

/// Боевая цепочка кадра (ввод, приказы, марши, бой). Системы, чьи данные нужны бою, ставятся `.before(CombatSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CombatSet;

pub struct DivisionPlugin;

impl Plugin for DivisionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectedDivision>()
            .init_resource::<OpenCombats>()
            .init_resource::<SelectedGroup>()
            .init_resource::<DragSelection>()
            .init_resource::<SelectedDivisions>()
            .init_resource::<StackView>()
            .init_resource::<DivisionTemplates>()
            .add_event::<OrderMove>()
            .add_event::<RefitDivision>()
            // метрики графа считаются сразу после того, как setup_map построил граф смежности
            .add_systems(Startup, (spawn_oob_divisions, init_graph_metrics.after(setup_map)))
            .add_systems(
                Update,
                (
                    select_division_input,
                    order_move_input,
                    apply_orders,
                    advance_marches,
                    resolve_combats,
                    update_division_positions,
                    scale_divisions_to_zoom,
                    update_selection_visual,
                    update_stack_visuals,
                    update_nato_icons,
                )
                    .chain()
                    .after(pick_province)
                    .after(ClockSet)
                    .in_set(CombatSet),
            )
            .add_systems(Update, (combat_ui, division_info_ui, stack_badges_ui, draw_selection_box))
            .add_systems(Update, daily_entrenchment.after(ClockSet))
            // выбор тактик — раз в сутки, до часового расчёта боя того же кадра
            .add_systems(Update, select_combat_tactics.after(ClockSet).before(CombatSet))
            // переформирование применяется до боевой цепочки кадра
            .add_systems(Update, apply_refits.before(CombatSet))
            .add_systems(Update, daily_supply.after(update_supply_network));
    }
}

// ---------------------------------------------------------------------------
// Спавн
// ---------------------------------------------------------------------------

fn spawn_division_entity(commands: &mut Commands, division: Division, center: Vec2, colors: &CountryColors) {
    let rgb = colors.map_rgb(&division.owner);
    let [r, g, b] = rgb;
    let symbol = division.nato_symbol;
    let line_color = symbol_line_color(rgb);

    // Корень — рамка; внутри заливка цветом нации, символ NATO, тёмная подложка и полоски орг. / прочности.
    commands
        .spawn((
            SpriteBundle {
                sprite: Sprite { color: FRAME_COLOR, custom_size: Some(DIVISION_SIZE), ..default() },
                transform: Transform::from_xyz(center.x, center.y, DIVISION_Z),
                ..default()
            },
            division,
            NatoIcon(symbol),
        ))
        .with_children(|p| {
            p.spawn(SpriteBundle {
                sprite: Sprite {
                    color: Color::srgb_u8(r, g, b),
                    custom_size: Some(DIVISION_SIZE - Vec2::splat(2.0)),
                    ..default()
                },
                transform: Transform::from_xyz(0.0, 0.0, 0.1),
                ..default()
            });
            // тёмно-серая подложка под полосками: сразу под NATO-прямоугольником (его высота 18 px)
            p.spawn(SpriteBundle {
                sprite: Sprite {
                    color: BAR_BACKDROP_COLOR,
                    custom_size: Some(Vec2::new(BAR_MAX_WIDTH, BAR_HEIGHT * 2.0)),
                    ..default()
                },
                transform: Transform::from_xyz(0.0, -DIVISION_SIZE.y * 0.5 - BAR_HEIGHT, 0.2),
                ..default()
            });
            spawn_nato_strokes(p, symbol, line_color);
            // полоски: организация (сверху) и прочность (снизу), растут вправо от левого края
            for (y, color, is_org) in [
                (-DIVISION_SIZE.y * 0.5 - BAR_HEIGHT * 0.5, ORG_BAR_COLOR, true),
                (-DIVISION_SIZE.y * 0.5 - BAR_HEIGHT * 1.5, STRENGTH_BAR_COLOR, false),
            ] {
                let bar = SpriteBundle {
                    sprite: Sprite {
                        color,
                        custom_size: Some(Vec2::new(BAR_MAX_WIDTH, BAR_HEIGHT)),
                        anchor: Anchor::CenterLeft,
                        ..default()
                    },
                    transform: Transform::from_xyz(-BAR_MAX_WIDTH * 0.5, y, 0.3),
                    ..default()
                };
                if is_org {
                    p.spawn((bar, OrgBar));
                } else {
                    p.spawn((bar, StrengthBar));
                }
            }
        });
}

pub(crate) fn is_water(map: &ProvinceMap, province: u32) -> bool {
    matches!(map.terrain_of(province), Some("ocean" | "lakes"))
}

/// Спавн дивизий из распарсенного OOB. Id назначаются по порядку в файлах (GER, затем FRA).
/// Дивизии на море или в провинциях без центра пропускаются.
fn spawn_oob_divisions(
    mut commands: Commands,
    oob: Res<OobDivisions>,
    centers: Res<ProvinceCenters>,
    map: Res<ProvinceMap>,
    registry: Res<StateRegistry>,
    templates: Res<DivisionTemplates>,
    colors: Res<CountryColors>,
) {
    let base = templates.get_or_base(BASE_TEMPLATE);
    let base_stats = base.calculate_stats();
    println!(
        "[ШАБЛОН] {}: {} бат., личный состав {}, прочность {}, организация {}, атака {}, защита {}, снаряжение {}",
        base.name,
        base.battalion_count(),
        base_stats.total_manpower,
        base_stats.max_strength,
        base_stats.max_org,
        base_stats.soft_attack,
        base_stats.defense,
        base_stats.total_equipment_needs()
    );

    let mut next_id = 1u32;
    let mut skipped = 0u32;
    // Счётчики по странам, порядок вывода детерминирован
    let mut per_owner: BTreeMap<&str, u32> = BTreeMap::new();

    // Провинция пригодна, если у неё есть центр и это не море
    let usable = |province: u32| centers.centers.contains_key(&province) && !is_water(&map, province);
    let mut relocated = 0u32;
    let mut remapped: BTreeMap<(String, String), u32> = BTreeMap::new();

    for entry in &oob.0 {
        let location = match resolve_placement(entry, &registry, &usable) {
            Placement::Exact(province) => province,
            Placement::Relocated { to, reason } => {
                eprintln!(
                    "[ПРЕДУПРЕЖДЕНИЕ] OOB {}: \"{}\" в провинции {}: {reason}; перенесена в {to}",
                    entry.owner, entry.name, entry.location
                );
                relocated += 1;
                to
            }
            Placement::Skipped(reason) => {
                eprintln!(
                    "[ПРЕДУПРЕЖДЕНИЕ] OOB {}: \"{}\" в провинции {}: {reason}; города для переноса нет, пропущена",
                    entry.owner, entry.name, entry.location
                );
                skipped += 1;
                continue;
            }
        };
        let Some(center) = centers.centers.get(&location) else {
            skipped += 1;
            continue;
        };
        // Шаблон из реестра нации; остальные сопоставляются с ближайшим доступным
        let template = resolve_template(&templates, &entry.owner, &entry.template);
        if template != entry.template {
            *remapped.entry((entry.template.clone(), template.to_string())).or_default() += 1;
        }
        let mut division = Division::from_template(
            next_id,
            entry.name.clone(),
            entry.owner.clone(),
            location,
            template.to_string(),
            templates.get_or_base(template),
        );
        division.experience_pct = entry.experience_pct;
        division.start_equipment_pct = entry.equipment_pct;
        spawn_division_entity(&mut commands, division, *center, &colors);
        *per_owner.entry(entry.owner.as_str()).or_default() += 1;
        next_id += 1;
    }

    for ((from, to), count) in &remapped {
        println!("[OOB] Шаблон \"{from}\" -> \"{to}\" ({count} шт.)");
    }
    for (tag, count) in &per_owner {
        println!("[OOB] Армия {tag}_1936 развёрнута: {count} дивизий размещено на карте.");
    }
    let breakdown: Vec<String> = per_owner.iter().map(|(tag, n)| format!("{tag} {n}")).collect();
    println!(
        "[OOB] Заспавнено дивизий: {} ({}), перенесено в крупнейший город: {relocated}, пропущено: {skipped}",
        next_id - 1,
        breakdown.join(", ")
    );
}

// ---------------------------------------------------------------------------
// Ввод (формирует только выбор / события)
// ---------------------------------------------------------------------------

/// ЛКМ: короткий клик по провинции выбирает весь стек стоящих в ней дивизий (активна дивизия с наименьшим
/// id, инспектор показывает список); протяжка длиннее порога выделяет рамкой все дивизии игрока, центры
/// которых попали в неё. Рамка без дивизий ведёт себя как обычный клик. Паник нет: нет окна, курсора или
/// проекции камеры — действие просто пропускается.
#[allow(clippy::too_many_arguments)]
fn select_division_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    hovered: Res<HoveredProvince>,
    player: Res<PlayerCountry>,
    divisions: Query<(Entity, &Division)>,
    transforms: Query<(Entity, &Division, &Transform)>,
    mut selected: ResMut<SelectedDivision>,
    mut group: ResMut<SelectedGroup>,
    mut multi: ResMut<SelectedDivisions>,
    mut drag: ResMut<DragSelection>,
    mut contexts: EguiContexts,
) {
    let cursor = windows.get_single().ok().and_then(|w| w.cursor_position());
    if mouse.just_pressed(MouseButton::Left) {
        let over_ui = contexts.try_ctx_mut().is_none_or(|ctx| ctx.wants_pointer_input());
        if over_ui {
            return;
        }
        if let Some(pos) = cursor {
            *drag = DragSelection { active: true, start_screen_pos: pos, current_screen_pos: pos, ..*drag };
        }
        return;
    }
    if !drag.active {
        return;
    }
    if let Some(pos) = cursor {
        drag.current_screen_pos = pos;
    }
    if !mouse.just_released(MouseButton::Left) {
        return;
    }
    drag.active = false;

    // Рамка: экранный прямоугольник -> мировой AABB через камеру
    if drag.is_box() {
        let corners = cameras.get_single().ok().and_then(|(camera, camera_transform)| {
            let a = camera.viewport_to_world_2d(camera_transform, drag.start_screen_pos)?;
            let b = camera.viewport_to_world_2d(camera_transform, drag.current_screen_pos)?;
            Some(normalized_rect(a, b))
        });
        if let Some((min, max)) = corners {
            let captured = boxed_divisions(
                transforms.iter().map(|(e, d, t)| (e, d.id, d.owner.as_str(), t.translation.truncate())),
                &player.0,
                min,
                max,
            );
            if let Some(&first) = captured.first() {
                selected.0 = Some(first);
                group.whole = true;
                multi.0 = captured;
                return;
            }
        }
    }

    // Обычный клик (или рамка без дивизий): выбор стека провинции под курсором
    multi.0.clear();
    group.whole = false;
    selected.0 = hovered.id.and_then(|province| {
        // Стек провинции: стоящие дивизии по возрастанию id — детерминированный порядок
        let standing = stack_layout(
            divisions.iter().filter(|(_, d)| d.current_province == province && d.movement.is_none()).map(|(e, d)| (e, d.id, province)),
            None,
        );
        if let Some(stack) = standing.get(&province) {
            group.whole = true;
            return Some(stack.members[0].1);
        }
        // Остались только идущие в марше (их фишки вне центра): выбираем с наименьшим id
        divisions
            .iter()
            .filter(|(_, d)| d.current_province == province)
            .min_by_key(|(_, d)| d.id)
            .map(|(e, _)| e)
    });
}

/// Рамка выделения: контрастная светло-зелёная граница и заливка с alpha ~15% в фоновом слое egui.
fn draw_selection_box(mut contexts: EguiContexts, drag: Res<DragSelection>) {
    if !drag.active || !drag.is_box() {
        return;
    }
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    let (min, max) = normalized_rect(drag.start_screen_pos, drag.current_screen_pos);
    let rect = egui::Rect::from_min_max(egui::pos2(min.x, min.y), egui::pos2(max.x, max.y));
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("selection_box")));
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(120, 255, 120, 38));
    painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(170, 255, 170)));
}

/// Приказы ПКМ после проверки суверенитета: вход в чужую нейтральную землю, море или неизвестную провинцию
/// не порождает ни одного `OrderMove`, причина пишется в консоль.
pub fn rmb_orders(
    diplomacy: &DiplomacyRegistry,
    player: &str,
    target_controller: Option<&str>,
    target: u32,
    units: &[Entity],
) -> Vec<OrderMove> {
    match check_entry(diplomacy, player, target_controller) {
        Ok(()) => units.iter().map(|&division| OrderMove { division, target }).collect(),
        Err(message) => {
            println!("{message}");
            Vec::new()
        }
    }
}

/// ПКМ (короткий клик без перетаскивания, т.к. ПКМ-drag — панорама камеры) -> `OrderMove`.
fn order_move_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    hovered: Res<HoveredProvince>,
    selected: Res<SelectedDivision>,
    group: Res<SelectedGroup>,
    multi: Res<SelectedDivisions>,
    divisions: Query<(Entity, &Division)>,
    player: Res<PlayerCountry>,
    controllers: Res<ProvinceControllers>,
    diplomacy: Option<Res<DiplomacyRegistry>>,
    mut orders: EventWriter<OrderMove>,
    mut press_pos: Local<Option<Vec2>>,
    mut contexts: EguiContexts,
) {
    let Ok(window) = windows.get_single() else { return };

    if mouse.just_pressed(MouseButton::Right) {
        *press_pos = window.cursor_position();
    }
    if !mouse.just_released(MouseButton::Right) {
        return;
    }
    let start = press_pos.take();
    if contexts.try_ctx_mut().is_none_or(|ctx| ctx.wants_pointer_input()) {
        return;
    }
    let (Some(start), Some(end)) = (start, window.cursor_position()) else { return };
    if start.distance(end) > CLICK_MAX_DRAG {
        return; // это была панорама
    }
    if let (Some(selected), Some(target)) = (selected.0, hovered.id) {
        // вся группа рамки, весь стек или одна дивизия, выбранная в инспекторе
        let units = command_targets(&multi, &divisions, selected, group.whole);
        let peace = DiplomacyRegistry::default();
        let diplomacy = diplomacy.as_deref().unwrap_or(&peace);
        for order in rmb_orders(diplomacy, &player.0, controllers.0.get(&target).map(String::as_str), target, &units) {
            orders.send(order);
        }
    }
}

// ---------------------------------------------------------------------------
// Симуляция (целые числа, часовые тики)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// A* по графу провинций
// ---------------------------------------------------------------------------

/// Целочисленный квадратный корень (floor). `f64::sqrt` корректно округляется по IEEE 754,
/// поправки в циклах гарантируют точный результат на любой платформе.
fn isqrt(n: u64) -> u64 {
    let mut r = (n as f64).sqrt() as u64;
    while r * r > n {
        r -= 1;
    }
    while (r + 1) * (r + 1) <= n {
        r += 1;
    }
    r
}

/// Центр провинции в целых пикселях.
fn center_i(centers: &ProvinceCenters, id: u32) -> Option<(i64, i64)> {
    centers.centers.get(&id).map(|c| (c.x.round() as i64, c.y.round() as i64))
}

fn dist_sq(a: (i64, i64), b: (i64, i64)) -> u64 {
    let dx = a.0.abs_diff(b.0);
    let dy = a.1.abs_diff(b.1);
    dx * dx + dy * dy
}

/// Евклидово расстояние, округлённое вниз (эвристика h — никогда не завышает).
fn dist_floor(a: (i64, i64), b: (i64, i64)) -> u32 {
    isqrt(dist_sq(a, b)) as u32
}

/// Евклидово расстояние, округлённое вверх.
fn dist_ceil(a: (i64, i64), b: (i64, i64)) -> u32 {
    let n = dist_sq(a, b);
    let r = isqrt(n);
    (if r * r < n { r + 1 } else { r }) as u32
}

/// Минимальное время перехода в любую провинцию (нижняя граница для эвристики).
const MIN_STEP_HOURS: u32 = 24;

/// Время перехода в провинцию с данной местностью, игровых часов.
pub fn terrain_hours(terrain: Option<&str>) -> u32 {
    match terrain {
        Some("plains" | "urban" | "desert") => 24,
        Some("forest" | "hills") => 36,
        Some("marsh") => 42,
        Some("mountain") => 60,
        _ => 24, // прочие типы и неизвестная местность
    }
}

/// Часы, нужные на проход `base_hours` (время для пехоты) дивизии со скоростью `speed` (км/ч × 10).
/// Масштаб обратно пропорционален скорости: 8.0 км/ч вдвое быстрее эталона 4.0. Округление вверх, минимум 1.
pub fn scale_hours(base_hours: u32, speed: u32) -> u32 {
    (u64::from(base_hours) * u64::from(INFANTRY_SPEED_KPH)).div_ceil(u64::from(speed.max(1))).max(1) as u32
}

/// Время перехода в провинцию с данной местностью для дивизии со скоростью `speed`.
pub fn march_hours(terrain: Option<&str>, speed: u32) -> u32 {
    scale_hours(terrain_hours(terrain), speed)
}

/// Предрассчитанные параметры графа провинций. Считаются один раз после построения графа смежности
/// (`init_graph_metrics`), поэтому поиск пути не обходит весь граф при каждом вызове.
#[derive(Resource, Debug, Clone, Copy)]
pub struct GraphMetrics {
    /// Самое длинное ребро графа: расстояние между центрами соседних провинций, округлено вверх.
    /// Нужно эвристике A*.
    pub max_hop_px: u32,
    /// Наибольший id провинции в графе: размер плотных массивов поиска.
    pub max_province_id: u32,
}

impl GraphMetrics {
    pub fn compute(adjacency: &ProvinceAdjacency, centers: &ProvinceCenters) -> Self {
        let mut max_hop_px = 0;
        let mut max_province_id = 0;
        for (&a, set) in &adjacency.neighbors {
            max_province_id = max_province_id.max(a);
            if let Some(&last) = set.iter().next_back() {
                max_province_id = max_province_id.max(last);
            }
            let Some(pa) = center_i(centers, a) else { continue };
            for &b in set.range(a + 1..) {
                // каждое ребро учитываем один раз (b > a)
                if let Some(pb) = center_i(centers, b) {
                    max_hop_px = max_hop_px.max(dist_ceil(pa, pb));
                }
            }
        }
        Self { max_hop_px, max_province_id }
    }
}

/// Один раз при старте, сразу после построения графа смежности в `setup_map`.
fn init_graph_metrics(mut commands: Commands, adjacency: Res<ProvinceAdjacency>, centers: Res<ProvinceCenters>) {
    let metrics = GraphMetrics::compute(&adjacency, &centers);
    println!(
        "[ГРАФ] Самое длинное ребро: {} px, максимальный id провинции: {}",
        metrics.max_hop_px, metrics.max_province_id
    );
    commands.insert_resource(metrics);
}

/// Начальная ёмкость очереди A*: поиск через всю карту посещает тысячи вершин.
const OPEN_CAPACITY: usize = 2048;
/// Отсутствие значения в плотных массивах поиска.
const NONE: u32 = u32::MAX;

/// A* от `start` к `goal` по `ProvinceAdjacency`; только суша. Возвращает путь включая обе точки.
/// Детерминизм: соседи идут по возрастанию id (BTreeSet), очередь — `BinaryHeap` с ключом
/// (f, id узла, g); оценки и предки лежат в плотных массивах по id провинции (без хэш-коллекций
/// и без `f32` в расчёте). Массивы и очередь выделяются заранее, а длина самого длинного ребра
/// берётся из предрассчитанных `GraphMetrics`.
/// Отличие от заданной сигнатуры: добавлены `map` (тип местности, вода) и `metrics`.
///
/// Суверенитет: провинция проходима, только если `diplomacy.can_enter_territory(owner, контроллер)`:
/// своя земля, земля страны, с которой война, или земля с правом прохода. Нейтральные страны и провинции
/// без контроллера — стена, поэтому дивизия в анклаве без сухопутного пути по своим землям получает `None`.
/// Стартовая провинция не проверяется (дивизия может стоять где угодно), а цель обязана быть проходимой.
#[allow(clippy::too_many_arguments)]
pub fn find_path_a_star(
    start: u32,
    goal: u32,
    adjacency: &ProvinceAdjacency,
    centers: &ProvinceCenters,
    map: &ProvinceMap,
    controllers: &ProvinceControllers,
    metrics: &GraphMetrics,
    owner: &str,
    diplomacy: &DiplomacyRegistry,
    speed: u32,
) -> Option<Vec<u32>> {
    let is_passable = |province: u32| {
        controllers.0.get(&province).is_some_and(|c| diplomacy.can_enter_territory(owner, c.as_str()))
    };
    if start == goal {
        return Some(vec![start]);
    }
    if is_water(map, goal) || !is_passable(goal) {
        return None;
    }
    let size = metrics.max_province_id as usize + 1;
    if start as usize >= size || goal as usize >= size {
        return None;
    }
    let goal_pos = center_i(centers, goal)?;
    let start_pos = center_i(centers, start)?;

    let mut open: BinaryHeap<Reverse<(u32, u32, u32)>> = BinaryHeap::with_capacity(OPEN_CAPACITY); // (f, node, g)
    let mut g_score = vec![NONE; size];
    let mut came_from = vec![NONE; size];

    // Эвристика в часах: до цели нужно минимум dist / max_hop переходов, каждый стоит >= MIN_STEP_HOURS.
    // Округление вниз, max_hop вверх — оценка никогда не завышает.
    let max_hop = u64::from(metrics.max_hop_px.max(1));
    // Быстрые дивизии проходят шаг быстрее, поэтому и нижняя граница шага масштабируется скоростью.
    let min_step = u64::from(scale_hours(MIN_STEP_HOURS, speed));
    let heuristic = |pos: (i64, i64)| -> u32 { (min_step * u64::from(dist_floor(pos, goal_pos)) / max_hop) as u32 };

    g_score[start as usize] = 0;
    open.push(Reverse((heuristic(start_pos), start, 0)));

    while let Some(Reverse((_, node, g))) = open.pop() {
        if g > g_score[node as usize] {
            continue; // устаревшая запись
        }
        if node == goal {
            let mut path = vec![goal];
            let mut cur = goal;
            while came_from[cur as usize] != NONE {
                cur = came_from[cur as usize];
                path.push(cur);
            }
            path.reverse();
            return Some(path);
        }
        let Some(neighbors) = adjacency.neighbors.get(&node) else { continue };
        for &n in neighbors {
            if n as usize >= size || is_water(map, n) || !is_passable(n) {
                continue;
            }
            let Some(n_pos) = center_i(centers, n) else { continue };
            // Стоимость шага — время перехода в целевую провинцию с учётом местности (часы).
            let tentative = g.saturating_add(march_hours(map.terrain_of(n), speed));
            if tentative < g_score[n as usize] {
                g_score[n as usize] = tentative;
                came_from[n as usize] = node;
                open.push(Reverse((tentative.saturating_add(heuristic(n_pos)), n, tentative)));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Симуляция (целые числа, часовые тики)
// ---------------------------------------------------------------------------

/// Приказ прокладывает маршрут A* и записывает его в `division.path`.
/// Сам марш стартует на ближайшем часовом тике (`advance_marches`).
fn apply_orders(
    mut events: EventReader<OrderMove>,
    mut divisions: Query<&mut Division>,
    combats: Query<&ActiveCombat>,
    adjacency: Res<ProvinceAdjacency>,
    centers: Res<ProvinceCenters>,
    map: Res<ProvinceMap>,
    controllers: Res<ProvinceControllers>,
    metrics: Res<GraphMetrics>,
    diplomacy: Option<Res<DiplomacyRegistry>>,
) {
    let peace = DiplomacyRegistry::default();
    let diplomacy = diplomacy.as_deref().unwrap_or(&peace);
    for order in events.read() {
        if combats.iter().any(|c| c.involves(order.division)) {
            println!("[ПРИКАЗ] Дивизия в бою — приказ отклонён");
            continue;
        }
        let Ok(mut division) = divisions.get_mut(order.division) else { continue };

        // Если дивизия уже в пути, новый маршрут строится от точки, куда она сейчас идёт.
        let start = division.movement.map_or(division.current_province, |m| m.to);
        // Путь идёт только по землям, куда дипломатия пускает владельца дивизии; нейтралы — стена.
        match find_path_a_star(
            start,
            order.target,
            &adjacency,
            &centers,
            &map,
            &controllers,
            &metrics,
            &division.owner,
            diplomacy,
            division.max_speed,
        ) {
            Some(path) => {
                division.path = path.into_iter().skip(1).collect(); // без стартовой точки
                if !division.path.is_empty() {
                    division.entrenchment = 0; // приказ на марш мгновенно сбрасывает окопанность
                }
                println!(
                    "[ПУТЬ] Дивизия #{}: {} -> {} ({} шагов)",
                    division.id,
                    start,
                    order.target,
                    division.path.len()
                );
            }
            None => println!(
                "[ПУТЬ] Дивизия #{}: путь {} -> {} не найден",
                division.id, start, order.target
            ),
        }
    }
}

/// Снимок дивизии для поиска противника, пока сами дивизии изменяются.
struct Snapshot {
    entity: Entity,
    owner: String,
    province: u32,
    stationary: bool,
}

/// Каждый `HourTick` уменьшает `hours_left`. По достижении 0 дивизия либо занимает провинцию,
/// либо (если там стоит вражеская дивизия) вступает в бой — тогда ждёт на границе с `hours_left == 0`.
fn advance_marches(
    mut ticks: EventReader<HourTick>,
    mut commands: Commands,
    mut divisions: Query<(Entity, &mut Division)>,
    mut combats: Query<&mut ActiveCombat>,
    adjacency: Res<ProvinceAdjacency>,
    map: Res<ProvinceMap>,
    mut controllers: ResMut<ProvinceControllers>,
    mut controller_changed: EventWriter<ProvinceControllerChanged>,
    rivers: Option<Res<ProvinceRivers>>,
) {
    for _ in ticks.read() {
        // Дивизии в бою (участвуют в уже существующих боях или созданных на этом тике).
        let mut busy: HashSet<Entity> =
            combats.iter().flat_map(|c| c.attackers.iter().chain(c.defenders.iter()).copied()).collect();
        // Участники боёв, существовавших на начало тика (нужны, чтобы не втянуть их в новый бой)
        let engaged = busy.clone();
        // Атакующие, дошедшие до вражеской провинции на этом тике: провинция -> атакующие (по id)
        let mut pending: BTreeMap<u32, Vec<Entity>> = BTreeMap::new();

        // Детерминированный порядок обхода — по id дивизии.
        let mut order: Vec<(u32, Entity)> = divisions.iter().map(|(e, d)| (d.id, e)).collect();
        order.sort_unstable();
        let mut snapshots: Vec<Snapshot> = order
            .iter()
            .filter_map(|&(_, e)| divisions.get(e).ok())
            .map(|(e, d)| Snapshot {
                entity: e,
                owner: d.owner.clone(),
                province: d.current_province,
                stationary: d.movement.is_none(),
            })
            .collect();

        for (_, entity) in order {
            let Ok((_, mut division)) = divisions.get_mut(entity) else { continue };
            if busy.contains(&entity) {
                continue; // в бою — движение по маршруту приостановлено
            }

            if let Some(mut m) = division.movement {
            if m.hours_left > 0 {
                m.hours_left -= 1;
                division.movement = Some(m);
            }
            if m.hours_left > 0 {
                continue; // ещё в пути
            }

            // Дошли до границы: ищем вражескую дивизию, стоящую в целевой провинции.
            let any_enemy = snapshots
                .iter()
                .any(|s| s.province == m.to && s.owner != division.owner && s.stationary);
            if any_enemy {
                // Вступаем в бой за провинцию (в конце тика): присоединяемся к идущему или начинаем новый.
                // До исхода боя дивизия стоит на границе с hours_left == 0, маршрут приостановлен.
                busy.insert(entity);
                pending.entry(m.to).or_default().push(entity);
                continue;
            }
            division.current_province = m.to;
            division.movement = None;
            if let Some(s) = snapshots.iter_mut().find(|s| s.entity == entity) {
                s.province = m.to;
                s.stationary = true;
            }
            println!("[МАРШ] Дивизия #{} прибыла в провинцию {}", division.id, m.to);

            // Бескровная оккупация: сюда попадаем только если врага в провинции нет (иначе выше начался бой).
            // Контроль меняется на том же часовом тике, что и завершение шага.
            if controllers.0.get(&m.to).is_some_and(|c| *c != division.owner) {
                controllers.0.insert(m.to, division.owner.clone());
                println!("[МАРШ-ОККУПАЦИЯ] Провинция #{} занята {} без боя", m.to, division.owner);
                controller_changed
                    .send(ProvinceControllerChanged { province_id: m.to, new_controller: division.owner.clone() });
            }
            }

            // Дивизия свободна: берём следующую точку маршрута и начинаем переход.
            if division.movement.is_none() && !division.path.is_empty() {
                let next = division.path.remove(0);
                let reachable = adjacency
                    .neighbors
                    .get(&division.current_province)
                    .is_some_and(|set| set.contains(&next));
                if reachable {
                    // Время шага зависит от местности целевой провинции
                    let hours = march_hours(map.terrain_of(next), division.max_speed);
                    division.entrenchment = 0; // начало шага марша (в т.ч. по приказу фронта / ИИ)
                    division.movement = Some(MovementProgress {
                        from: division.current_province,
                        to: next,
                        hours_left: hours,
                        total_hours: hours,
                    });
                    if let Some(s) = snapshots.iter_mut().find(|s| s.entity == entity) {
                        s.stationary = false;
                    }
                } else {
                    division.path.clear(); // маршрут устарел (например, дивизию отбросили)
                }
            }
        }

        // Вступление в бой. Защитники нового боя — все стоящие в провинции вражеские дивизии,
        // ещё не втянутые в другие бои; «лишние» сверх ширины фронта попадут в резерв в `resolve_combats`.
        for (province, attackers) in pending {
            let Some(owner) = snapshots.iter().find(|s| s.entity == attackers[0]).map(|s| s.owner.clone()) else {
                continue;
            };
            if let Some(mut combat) =
                combats.iter_mut().find(|c| c.province_id == province && c.attacker_owner == owner)
            {
                for attacker in attackers {
                    if !combat.attackers.contains(&attacker) {
                        combat.attackers.push(attacker);
                    }
                }
                continue;
            }
            let defenders: Vec<Entity> = snapshots
                .iter()
                .filter(|s| s.province == province && s.owner != owner && s.stationary && !engaged.contains(&s.entity))
                .map(|s| s.entity)
                .collect();
            let Some(defender_owner) = defenders
                .first()
                .and_then(|d| snapshots.iter().find(|s| s.entity == *d))
                .map(|s| s.owner.clone())
            else {
                continue; // все защитники уже заняты другими боями — атакующие повторят попытку на следующем тике
            };
            println!(
                "[БОЙ] Атака на провинцию {province}: атакующих {}, защитников {}",
                attackers.len(),
                defenders.len()
            );
            // Бой через реку начинается в речной фазе: захват против удержания моста
            let river_crossing = rivers.as_deref().is_some_and(|r| {
                attackers
                    .iter()
                    .filter_map(|&e| divisions.get(e).ok())
                    .any(|(_, d)| d.movement.is_some_and(|m| r.crosses(m.from, province)))
            });
            let (phase, phase_hours_left, attacker_tactic, defender_tactic) = if river_crossing {
                (CombatPhase::Bridge, BRIDGE_PHASE_HOURS, CombatTactic::SeizeBridge, CombatTactic::HoldBridge)
            } else {
                (CombatPhase::Default, 0, CombatTactic::Attack, CombatTactic::Defend)
            };
            let (is_attacker_countered, is_defender_countered) = resolve_counters(attacker_tactic, defender_tactic);
            commands.spawn(ActiveCombat {
                province_id: province,
                attacker_owner: owner,
                defender_owner,
                attackers,
                defenders,
                max_width: BASE_COMBAT_WIDTH,
                combat_progress: 0,
                elapsed_hours: 0,
                attacker_tactic,
                defender_tactic,
                is_attacker_countered,
                is_defender_countered,
                tactic_change_day: 0,
                phase,
                phase_hours_left,
                bridge_held: false,
            });
        }
    }
}

/// Номер игровых суток для детерминированных бросков (календарь упрощён: 12 месяцев по 31 дню).
pub fn day_index(day: u32, month: u32, year: u32) -> u32 {
    year.saturating_mul(372).saturating_add(month.saturating_mul(31)).saturating_add(day)
}

/// Выбор тактик обеих сторон на сутки. Сторона с худшей разведкой выбирает первой (равенство — атакующие),
/// вторая видит выбор и даёт контр-пику двойной вес. Бросок — `det_rand(боя, суток, ключа командующего)`.
///
/// В городе (`urban`) вес штурма у атакующих утраивается.
pub fn choose_tactics(
    combat_bits: u64,
    day: u32,
    attacker_key: u32,
    defender_key: u32,
    recon_edge: Option<bool>,
    urban: bool,
    breakthrough: bool,
) -> (CombatTactic, CombatTactic) {
    let scale = urban.then_some((CombatTactic::Assault, URBAN_ASSAULT_WEIGHT_MULT));
    // Прорыв входит в пул атакующих только при выполнении условий, а засада — как контр-пик обороны
    let (attackers, defenders): (&[CombatTactic], &[CombatTactic]) = if breakthrough {
        (&ATTACKER_TACTICS_WITH_BREAKTHROUGH, &DEFENDER_TACTICS_WITH_AMBUSH)
    } else {
        (&ATTACKER_TACTICS, &DEFENDER_TACTICS)
    };
    choose_from_pools((combat_bits, day, attacker_key, defender_key, recon_edge), (attackers, defenders, scale), [1, 2, 3, 4])
}

/// Суточный переход в фазу прорыва: не парированный прорыв включает (или продлевает) фазу на 24 часа; по истечении
/// срока без нового прорыва — обычная фаза; пока срок идёт, фаза доживает своё время.
pub fn next_breakthrough_phase(
    phase: CombatPhase,
    hours_left: u32,
    attacker: CombatTactic,
    attacker_countered: bool,
) -> (CombatPhase, u32) {
    if attacker == CombatTactic::Breakthrough && !attacker_countered {
        (CombatPhase::Breakthrough, BREAKTHROUGH_PHASE_HOURS)
    } else if phase == CombatPhase::Breakthrough && hours_left == 0 {
        (CombatPhase::Default, 0)
    } else {
        (phase, hours_left)
    }
}

/// Выбор тактик в фазе ближнего боя: атака в упор или продление штурма против обороны или разрыва дистанции.
pub fn choose_cq_tactics(
    combat_bits: u64,
    day: u32,
    attacker_key: u32,
    defender_key: u32,
    recon_edge: Option<bool>,
) -> (CombatTactic, CombatTactic) {
    choose_from_pools(
        (combat_bits, day, attacker_key, defender_key, recon_edge),
        (&CQ_ATTACKER_TACTICS, &CQ_DEFENDER_TACTICS, None),
        [9, 10, 11, 12],
    )
}

/// Общий порядок выбора: сторона с худшей разведкой первой, вторая видит выбор и ценит контр-пик вдвое выше.
fn choose_from_pools(
    (combat_bits, day, attacker_key, defender_key, recon_edge): (u64, u32, u32, u32, Option<bool>),
    (attacker_pool, defender_pool, attacker_scale): (&[CombatTactic], &[CombatTactic], Option<(CombatTactic, u64)>),
    salts: [u64; 4],
) -> (CombatTactic, CombatTactic) {
    let roll = |key: u32, salt: u64| det_rand(combat_bits, u64::from(day), u64::from(key) | (salt << 32));
    if recon_edge == Some(true) {
        // у атакующих разведка лучше: защитники выбирают первыми
        let defender = pick_tactic_scaled(defender_pool, None, roll(defender_key, salts[0]), None);
        let attacker = pick_tactic_scaled(attacker_pool, Some(defender), roll(attacker_key, salts[1]), attacker_scale);
        (attacker, defender)
    } else {
        let attacker = pick_tactic_scaled(attacker_pool, None, roll(attacker_key, salts[2]), attacker_scale);
        let defender = pick_tactic_scaled(defender_pool, Some(attacker), roll(defender_key, salts[3]), None);
        (attacker, defender)
    }
}

/// Суточный переход в ближний бой: штурм в городе включает (или продлевает) фазу на 24 часа; по истечении срока
/// без нового штурма фаза возвращается в обычную; пока срок идёт, она доживает своё время.
pub fn next_cq_phase(phase: CombatPhase, hours_left: u32, attacker: CombatTactic, urban: bool) -> (CombatPhase, u32) {
    if urban && attacker == CombatTactic::Assault {
        (CombatPhase::CloseQuarters, CLOSE_QUARTERS_PHASE_HOURS)
    } else if phase == CombatPhase::CloseQuarters && hours_left == 0 {
        (CombatPhase::Default, 0)
    } else {
        (phase, hours_left)
    }
}

/// Выбор речных тактик на сутки: защитники берут удержание или контрудар, наступающие — захват моста
/// (вес ×2, если среди них есть сапёры). Порядок выбора по разведке и броски — как у обычных тактик.
pub fn choose_bridge_tactics(
    combat_bits: u64,
    day: u32,
    attacker_key: u32,
    defender_key: u32,
    recon_edge: Option<bool>,
    engineers: bool,
) -> (CombatTactic, CombatTactic) {
    let roll = |key: u32, salt: u64| det_rand(combat_bits, u64::from(day), u64::from(key) | (salt << 32));
    let boost = engineers.then_some(CombatTactic::SeizeBridge);
    if recon_edge == Some(true) {
        let defender = pick_tactic_boosted(&BRIDGE_DEFENDER_TACTICS, None, roll(defender_key, 5), None);
        let attacker = pick_tactic_boosted(&BRIDGE_ATTACKER_TACTICS, Some(defender), roll(attacker_key, 6), boost);
        (attacker, defender)
    } else {
        let attacker = pick_tactic_boosted(&BRIDGE_ATTACKER_TACTICS, None, roll(attacker_key, 7), boost);
        let defender = pick_tactic_boosted(&BRIDGE_DEFENDER_TACTICS, Some(attacker), roll(defender_key, 8), None);
        (attacker, defender)
    }
}

/// Мост у атакующих после выбора тактик: захват, не парированный удержанием, снимает речной штраф.
/// Контрудар защитников выбивает атакующих с моста, который они уже держали.
pub fn bridge_held_after(previously_held: bool, attacker: CombatTactic, defender: CombatTactic) -> bool {
    let (attacker_countered, _) = resolve_counters(attacker, defender);
    let seized = attacker == CombatTactic::SeizeBridge && !attacker_countered;
    seized && !(previously_held && defender == CombatTactic::RetakeBridge)
}

/// Выход из речной фазы: когда срок вышел и мост у атакующих, бой переходит в обычную фазу (штраф остаётся снят);
/// если мост не взят, фаза продолжается ещё на срок. Пока срок идёт, ничего не меняется.
pub fn next_bridge_phase(bridge_held: bool, hours_left: u32) -> (CombatPhase, u32) {
    if hours_left > 0 {
        (CombatPhase::Bridge, hours_left)
    } else if bridge_held {
        (CombatPhase::Default, 0)
    } else {
        (CombatPhase::Bridge, BRIDGE_PHASE_HOURS)
    }
}

/// Суточный переход фазы боя: отход защитника включает (или продлевает) фазу на 24 часа; фаза, срок которой
/// истёк, а отход не выбран, возвращается в обычную; пока срок идёт, фаза доживает своё время.
pub fn next_phase(phase: CombatPhase, hours_left: u32, defender: CombatTactic) -> (CombatPhase, u32) {
    if defender == CombatTactic::TacticalWithdrawal {
        (CombatPhase::TacticalWithdrawal, WITHDRAWAL_PHASE_HOURS)
    } else if phase == CombatPhase::TacticalWithdrawal && hours_left == 0 {
        (CombatPhase::Default, 0)
    } else {
        (phase, hours_left)
    }
}

/// Раз в сутки (00:00) командующие обеих сторон каждого боя выбирают тактики; парирование выставляет
/// флаги `is_*_countered`. Ключ командующего — наименьший id дивизии стороны (у дивизий нет ссылки на
/// генерала, а состав стороны в бою детерминирован).
fn select_combat_tactics(
    mut events: EventReader<DailyTickEvent>,
    mut combats: Query<(Entity, &mut ActiveCombat)>,
    divisions: Query<&Division>,
    map: Res<ProvinceMap>,
) {
    for event in events.read() {
        let day = day_index(event.day, event.month, event.year);
        for (entity, mut combat) in &mut combats {
            let recon_of = |side: &[Entity]| -> Vec<u32> {
                let alive: Vec<&Division> = side.iter().filter_map(|&e| divisions.get(e).ok()).collect();
                let line: Vec<u32> = alive.iter().filter(|d| d.combat_state == CombatState::Active).map(|d| d.recon).collect();
                if line.is_empty() { alive.iter().map(|d| d.recon).collect() } else { line }
            };
            let key_of = |side: &[Entity]| side.iter().filter_map(|&e| divisions.get(e).ok()).map(|d| d.id).min().unwrap_or(0);
            let edge = recon_advantage(&recon_of(&combat.attackers), &recon_of(&combat.defenders));
            let in_bridge_phase = combat.phase == CombatPhase::Bridge;
            let urban = map.terrain_of(combat.province_id) == Some("urban");
            let (attacker, defender) = if in_bridge_phase {
                // сапёры среди атакующих повышают вес захвата моста
                let engineers = combat
                    .attackers
                    .iter()
                    .filter_map(|&e| divisions.get(e).ok())
                    .any(|d| d.river_penalty_reduction > 0);
                choose_bridge_tactics(
                    entity.to_bits(),
                    day,
                    key_of(&combat.attackers),
                    key_of(&combat.defenders),
                    edge,
                    engineers,
                )
            } else if combat.phase == CombatPhase::CloseQuarters {
                choose_cq_tactics(entity.to_bits(), day, key_of(&combat.attackers), key_of(&combat.defenders), edge)
            } else {
                // Условия прорыва: средняя твёрдость атакующих или превосходство командующего
                let attackers: Vec<&Division> = combat.attackers.iter().filter_map(|&e| divisions.get(e).ok()).collect();
                let defenders: Vec<&Division> = combat.defenders.iter().filter_map(|&e| divisions.get(e).ok()).collect();
                let avg_hardness = if attackers.is_empty() {
                    0
                } else {
                    attackers.iter().map(|d| d.hardness).sum::<u32>() / attackers.len() as u32
                };
                let skill = |side: &[&Division]| side.iter().map(|d| d.command_attack_skill + d.command_defense_skill).max().unwrap_or(0);
                let breakthrough = breakthrough_available(
                    avg_hardness,
                    skill(&attackers),
                    skill(&defenders),
                    key_of(&combat.attackers),
                    key_of(&combat.defenders),
                );
                choose_tactics(
                    entity.to_bits(),
                    day,
                    key_of(&combat.attackers),
                    key_of(&combat.defenders),
                    edge,
                    urban,
                    breakthrough,
                )
            };
            let (att_countered, def_countered) = resolve_counters(attacker, defender);
            combat.attacker_tactic = attacker;
            combat.defender_tactic = defender;
            combat.is_attacker_countered = att_countered;
            combat.is_defender_countered = def_countered;
            combat.tactic_change_day = day;
            // Фаза отхода: выбор отхода защитником включает её на 24 часа; по истечении срока без отхода — обычная
            if in_bridge_phase {
                combat.bridge_held = bridge_held_after(combat.bridge_held, attacker, defender);
                let (phase, hours_left) = next_bridge_phase(combat.bridge_held, combat.phase_hours_left);
                combat.phase = phase;
                combat.phase_hours_left = hours_left;
            } else {
                let (phase, hours_left) = next_phase(combat.phase, combat.phase_hours_left, defender);
                // штурм города имеет приоритет над отходом: уличный бой сменяет прочие фазы
                let (phase, hours_left) = next_cq_phase(phase, hours_left, attacker, urban);
                // успешный прорыв сменяет отход, но не уличный бой (штурм города и прорыв не выбираются вместе)
                let (phase, hours_left) = next_breakthrough_phase(phase, hours_left, attacker, combat.is_attacker_countered);
                combat.phase = phase;
                combat.phase_hours_left = hours_left;
            }
        }
    }
}

/// Строка тактик для интерфейса: сегменты `(текст, парирована)`; парированная сторона получает пометку.
pub fn tactics_segments(combat: &ActiveCombat) -> Vec<(String, bool)> {
    let (att_countered, def_countered) = resolve_counters(combat.attacker_tactic, combat.defender_tactic);
    let mut segments = vec![("Тактика: ".to_string(), false)];
    segments.push((combat.attacker_tactic.title().to_string(), false));
    if att_countered {
        segments.push((" [ПАРИРОВАНО!]".to_string(), true));
    }
    segments.push((" vs ".to_string(), false));
    segments.push((combat.defender_tactic.title().to_string(), false));
    if def_countered {
        segments.push((" [ПАРИРОВАНО!]".to_string(), true));
    }
    segments
}

/// Статус фазы боя для интерфейса; в обычной фазе — `None`.
pub fn phase_text(combat: &ActiveCombat) -> Option<String> {
    match combat.phase {
        CombatPhase::Default => None,
        CombatPhase::TacticalWithdrawal => Some(format!(
            "Фаза: {} (осталось {} ч.) | Фронт сужен на 50%",
            combat.phase.title(),
            combat.phase_hours_left
        )),
        CombatPhase::CloseQuarters => Some(format!(
            "Фаза: {} (осталось {} ч.) | Фронт сужен на 50% | Урон повышен",
            combat.phase.title(),
            combat.phase_hours_left
        )),
        CombatPhase::Breakthrough => Some(format!(
            "Фаза: {} (осталось {} ч.) | Окопанность врага урезана на 50%",
            combat.phase.title(),
            combat.phase_hours_left
        )),
        CombatPhase::Bridge => Some(format!(
            "Фаза: {} | {}",
            combat.phase.title(),
            if combat.bridge_held { "Мост захвачен (штраф реки снят)" } else { "Мост удерживается обороной" }
        )),
    }
}

/// Показывает строку тактик: красная пометка у парированной стороны.
fn show_tactics(ui: &mut egui::Ui, combat: &ActiveCombat) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (text, countered) in tactics_segments(combat) {
            if countered {
                ui.label(egui::RichText::new(text).small().color(egui::Color32::from_rgb(230, 60, 60)).strong());
            } else {
                ui.label(egui::RichText::new(text).small());
            }
        }
    });
    if let Some(phase) = phase_text(combat) {
        ui.label(egui::RichText::new(phase).small().color(egui::Color32::from_rgb(240, 190, 80)));
    }
}

/// Тактическое преимущество стороны с лучшей разведкой: +5% к наносимому урону.
pub const TACTICAL_ADVANTAGE_PCT: u32 = 5;

/// Сторона с более высокой средней разведкой на линии боя: `Some(true)` — атакующие,
/// `Some(false)` — защитники, `None` — равенство или пустая линия. Средние сравниваются без деления
/// (`сумма_а * число_в` против `сумма_в * число_а`), поэтому дробные различия не теряются.
pub fn recon_advantage(attackers: &[u32], defenders: &[u32]) -> Option<bool> {
    if attackers.is_empty() || defenders.is_empty() {
        return None;
    }
    let sum = |side: &[u32]| side.iter().map(|&r| u64::from(r)).sum::<u64>();
    let (left, right) = (sum(attackers) * defenders.len() as u64, sum(defenders) * attackers.len() as u64);
    match left.cmp(&right) {
        std::cmp::Ordering::Greater => Some(true),
        std::cmp::Ordering::Less => Some(false),
        std::cmp::Ordering::Equal => None,
    }
}

/// Шанс (%) выхода дивизии из резерва за час: 2..=5 по id, вдвое выше у стороны с преимуществом в разведке.
fn reserve_chance_pct(division_id: u32, has_recon_advantage: bool) -> u64 {
    let base = u64::from(RESERVE_MIN_CHANCE_PCT + division_id % 4);
    if has_recon_advantage { base * 2 } else { base }
}

/// Штраф к атаке при форсировании реки (между провинцией атакующего и обороняемой есть река), %.
pub const RIVER_PENALTY_PCT: u32 = 30;
/// Твёрдость, выше которой часть считается бронированной (для штрафов горам и болоту).
const ARMORED_HARDNESS_PCT: u32 = 50;

/// Боевые модификаторы местности обороняемой провинции (проценты).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainModifiers {
    /// Бонус к эффективной защите обороняющихся.
    pub defense_bonus: u32,
    /// Штраф к атаке наступающих.
    pub attack_penalty: u32,
    /// Дополнительный штраф к атаке наступающих бронированных частей (`hardness > 50`).
    pub armored_attack_penalty: u32,
}

/// Модификаторы по типу местности (названия из `definition.csv`). Остальные типы — без модификаторов.
pub fn terrain_modifiers(terrain: Option<&str>) -> TerrainModifiers {
    let (defense_bonus, attack_penalty, armored_attack_penalty) = match terrain {
        Some("forest") => (20, 10, 0),
        Some("hills") => (25, 15, 0),
        Some("mountain") => (50, 30, 20),
        Some("marsh") => (20, 25, 30),
        Some("urban") => (30, 30, 20),
        _ => (0, 0, 0), // plains и прочие типы
    };
    TerrainModifiers { defense_bonus, attack_penalty, armored_attack_penalty }
}

/// Штраф форсирования реки для дивизии: -30%, с сапёрами -10%.
pub fn river_penalty_for(division: &Division) -> u32 {
    RIVER_PENALTY_PCT.saturating_sub(division.river_penalty_reduction)
}

/// Условия конкретного удара: штраф атаки бьющего и бонус защиты цели, %.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitContext {
    pub attack_penalty: u32,
    pub defense_bonus: u32,
    /// Бонус к атаке бьющего от превосходства в воздухе (до +10%).
    pub air_attack_bonus: u32,
    /// Штраф к защите цели под вражеским воздушным давлением (до -15%).
    pub air_defense_penalty: u32,
    /// Бонус к атаке бьющего от его тактики, %.
    pub tactic_attack_pct: u32,
    /// Бонус к защите цели от её тактики, %.
    pub tactic_defense_pct: u32,
    /// Делитель окопанности цели: 1 в обычном бою, 2 в фазе прорыва (укрепления прорваны).
    pub entrenchment_divisor: u32,
}

impl HitContext {
    /// Без модификаторов (удары обороняющихся и тесты без рельефа).
    pub const NONE: HitContext =
        HitContext {
        attack_penalty: 0,
        defense_bonus: 0,
        air_attack_bonus: 0,
        air_defense_penalty: 0,
        tactic_attack_pct: 0,
        tactic_defense_pct: 0,
        entrenchment_divisor: 1,
    };

    /// Делит окопанность цели (фаза прорыва): эффективная окопанность = окопанность / делитель.
    pub fn with_entrenchment_divisor(self, divisor: u32) -> Self {
        HitContext { entrenchment_divisor: divisor.max(1), ..self }
    }

    /// Добавляет эффекты тактик: бонус атаки бьющего и бонус защиты цели, %.
    pub fn with_tactics(self, attack_pct: u32, defense_pct: u32) -> Self {
        HitContext { tactic_attack_pct: attack_pct, tactic_defense_pct: defense_pct, ..self }
    }

    /// Добавляет влияние неба: бонус атаки бьющему и штраф защиты цели, %.
    pub fn with_air(self, attack_bonus: u32, defense_penalty: u32) -> Self {
        HitContext { air_attack_bonus: attack_bonus, air_defense_penalty: defense_penalty, ..self }
    }

    /// Удар наступающей дивизии: штраф рельефа (бронированным — повышенный) и реки, защитникам — бонус рельефа.
    pub fn for_attack(terrain: &TerrainModifiers, striker: &Division, over_river: bool) -> Self {
        let armored = if striker.hardness > ARMORED_HARDNESS_PCT { terrain.armored_attack_penalty } else { 0 };
        let river = if over_river { river_penalty_for(striker) } else { 0 };
        HitContext {
            attack_penalty: terrain.attack_penalty + armored + river,
            defense_bonus: terrain.defense_bonus,
            ..HitContext::NONE
        }
    }
}

/// Базовая атака дивизии по цели с твёрдостью 	arget_hardness (0..=100 %):
/// (soft * (100 - твёрдость) + hard * твёрдость) / 100 — целочисленно, сумма делится один раз (без потери точности).
/// Дальше к ней применяются укомплектованность, планирование, командиры, рельеф, реки, небо и разведка.
pub fn blended_attack(att: &Division, target_hardness: u32) -> u32 {
    let hardness = target_hardness.min(100);
    (att.soft_attack * (100 - hardness) + att.hard_attack * hardness) / 100
}

/// Интенсивность удара без модификаторов рельефа и неба (для тестов формулы).
#[cfg(test)]
fn combat_bp(att: &Division, def: &Division, planning: u32) -> u32 {
    combat_bp_ctx(att, def, planning, HitContext::NONE)
}

/// Часовая интенсивность удара `att` по `def` в bp от организации цели (целые числа).
/// Атака и защита пропорциональны укомплектованности (current_strength / max_strength).
/// `planning` — бонус планирования атакующего в процентах: `attack * (100 + planning) / 100`.
/// `ctx` — рельеф и река: `attack * (100 - штраф) / 100`, `defense * (100 + бонус) / 100`.
fn combat_bp_ctx(att: &Division, def: &Division, planning: u32, ctx: HitContext) -> u32 {
    // Сила удара зависит от твёрдости цели: мягкая атака против живой силы, жёсткая — против техники.
    let base_attack = blended_attack(att, def.hardness) * att.current_strength / att.max_strength.max(1);
    // Опыт атакующей дивизии: от -25% (новобранцы) до +75% (ветераны)
    let base_attack = base_attack * att.experience_multiplier_pct() / 100;
    let mut attack = base_attack * (100 + planning.min(MAX_PLANNING)) / 100;
    // Навык командира фронта: +2% к атаке за уровень; сверх лимита командира — вдвое слабее
    attack = attack * (100 + att.command_attack_skill * SKILL_BONUS_PCT) / 100;
    if att.over_command_limit {
        attack = attack * 50 / 100;
    }
    if !att.is_supplied {
        attack = attack * UNSUPPLIED_STAT_PCT / 100; // отрезан от снабжения: -33%
    }

    // Рельеф обороняемой провинции и переправа через реку режут атаку наступающих
    attack = attack * (100 - ctx.attack_penalty.min(100)) / 100;
    attack = attack * (100 + ctx.air_attack_bonus) / 100; // превосходство в воздухе: до +10%
    attack = attack * (100 + ctx.tactic_attack_pct) / 100; // тактика командующего

    let base_defense = def.defense * def.current_strength / def.max_strength.max(1);
    // Опыт обороняющейся дивизии масштабирует защиту так же, как атаку
    let base_defense = base_defense * def.experience_multiplier_pct() / 100;
    // Окопанность: +2% защиты за пункт (у наступающих она всегда 0 — марш её сбрасывает)
    let entrenchment = def.entrenchment.min(def.max_entrenchment) / ctx.entrenchment_divisor.max(1);
    let mut defense = base_defense * (100 + entrenchment * 2) / 100;
    defense = defense * (100 + def.command_defense_skill * SKILL_BONUS_PCT) / 100;
    if def.over_command_limit {
        defense = defense * 50 / 100;
    }
    if !def.is_supplied {
        defense = defense * UNSUPPLIED_STAT_PCT / 100; // отрезан от снабжения: -33%
    }
    defense = defense * (100 + ctx.defense_bonus) / 100; // бонус рельефа обороняющимся
    defense = defense * (100 - ctx.air_defense_penalty.min(100)) / 100; // вражеская авиация: до -15%
    defense = defense * (100 + ctx.tactic_defense_pct) / 100; // тактика командующего
    (COMBAT_INTENSITY_BP * attack / (attack + defense).max(1)).max(1)
}

/// Правило брони для удара `striker` по `target` (целые числа):
/// - цель бронирована сильнее, чем пробитие бьющего (`armor(target) > piercing(striker)`) —
///   урон вдвое меньше: пехота не пробивает броню;
/// - бьющий бронирован сильнее, чем пробитие цели (`armor(striker) > piercing(target)`) —
///   урон на 50% больше: безнаказанный расстрел с позиций.
/// Для пары «A атакует B» это ровно условие `armor(B) > piercing(A)`: удар A по B режется вдвое,
/// ответный удар B по A усиливается; правило симметрично и работает, если бронирован атакующий.
///
/// В упор (`point_blank`, фаза ближнего боя) броня преимущества не даёт ни той, ни другой стороне.
fn apply_armor_rule(damage: u32, striker: &Division, target: &Division, point_blank: bool) -> u32 {
    if point_blank {
        return damage;
    }
    let mut damage = damage;
    if target.armor > striker.piercing {
        damage /= 2;
    }
    if striker.armor > target.piercing {
        damage = damage * 150 / 100;
    }
    damage
}

/// Детерминированный хэш (splitmix64) — «случайность» без зависимости от кадров и платформы.
fn det_rand(a: u64, b: u64, c: u64) -> u64 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.rotate_left(21) ^ c.rotate_left(42);
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

enum Outcome {
    Continue,
    AttackerWins,
    AttackerLoses,
}

/// Дивизии стороны: (id, сущность, ширина, состояние), по возрастанию id.
fn side_info(divisions: &Query<(Entity, &mut Division)>, list: &[Entity]) -> Vec<(u32, Entity, u32, CombatState)> {
    let mut info: Vec<_> = list
        .iter()
        .filter_map(|&e| divisions.get(e).ok().map(|(_, d)| (d.id, e, d.combat_width, d.combat_state)))
        .collect();
    info.sort_unstable_by_key(|&(id, ..)| id);
    info
}

fn set_combat_state(divisions: &mut Query<(Entity, &mut Division)>, entity: Entity, state: CombatState) {
    if let Ok((_, mut d)) = divisions.get_mut(entity) {
        d.combat_state = state;
    }
}

/// Кто где стоит: (владелец, провинция) всех живых дивизий — для проверки безопасности отступления.
fn occupancy_snapshot(divisions: &Query<(Entity, &mut Division)>, despawned: &HashSet<Entity>) -> Vec<(String, u32)> {
    divisions
        .iter()
        .filter(|(e, _)| !despawned.contains(e))
        .map(|(_, d)| (d.owner.clone(), d.current_province))
        .collect()
}

/// Куда можно отступить: смежная суша под контролем своей страны, где нет чужих дивизий.
/// Порядок кандидатов — по возрастанию id (BTreeSet), поэтому выбор детерминирован.
fn safe_retreat_provinces(
    province: u32,
    owner: &str,
    adjacency: &ProvinceAdjacency,
    map: &ProvinceMap,
    controllers: &ProvinceControllers,
    occupancy: &[(String, u32)],
) -> Vec<u32> {
    adjacency
        .neighbors
        .get(&province)
        .into_iter()
        .flatten()
        .copied()
        .filter(|&n| !is_water(map, n) && controllers.0.get(&n).is_some_and(|c| c == owner))
        .filter(|&n| !occupancy.iter().any(|(o, p)| *p == n && o != owner))
        .collect()
}

/// Отступление дивизий в одну из `candidates` либо капитуляция в котле, если безопасных провинций нет.
/// `units` — (id, сущность); `seed` — источник детерминированного выбора.
#[allow(clippy::too_many_arguments)]
fn retreat_or_capitulate(
    units: &[(u32, Entity)],
    province: u32,
    candidates: &[u32],
    seed: u64,
    commands: &mut Commands,
    divisions: &mut Query<(Entity, &mut Division)>,
    despawned: &mut HashSet<Entity>,
    selected: &mut SelectedDivision,
) {
    let Some(&(first_id, _)) = units.first() else { return };
    if candidates.is_empty() {
        // Котёл: безопасной соседней провинции нет — дивизии капитулируют
        for &(_, e) in units {
            // Каждый корень удаляется ровно один раз; дубли отсекаются набором
            if !despawned.insert(e) {
                continue;
            }
            if let Ok((_, d)) = divisions.get(e) {
                println!("[КОТЕЛ] Дивизия {} окружена и капитулировала!", d.name);
            }
            // despawn_recursive сам удаляет дочерние спрайты (рамка, бейдж)
            commands.entity(e).despawn_recursive();
            if selected.0 == Some(e) {
                selected.0 = None;
            }
        }
        return;
    }
    // Все отступают в одну провинцию, выбранную детерминированно
    let idx = (det_rand(province as u64, first_id as u64, seed) % candidates.len() as u64) as usize;
    let retreat = candidates[idx];
    for &(_, e) in units {
        if let Ok((_, mut d)) = divisions.get_mut(e) {
            d.current_province = retreat;
            d.entrenchment = 0; // отступление сбрасывает окопанность
            d.movement = None;
            d.path.clear();
            d.combat_state = CombatState::None;
        }
    }
    println!("[БОЙ] Отступило дивизий: {} из провинции {province} в провинцию {retreat}", units.len());
}

/// Бой на каждом часовом тике (целые числа, порядок по провинции и id):
/// 1. к защите присоединяется весь гарнизон провинции; пересчитывается лимит ширины;
/// 2. новые участники занимают линию, пока хватает ширины, остальные уходят в резерв;
/// 3. дивизии линии бьют друг друга по кругу (резерв урон не наносит и не получает);
/// 4. дивизии линии с нулевой организацией выходят из боя и отступают поодиночке;
/// 5. резерв с шансом 2..=5% в час занимает освободившуюся ширину;
/// 6. сторона без дивизий на линии проигрывает бой: защита отступает, атакующий занимает провинцию.
#[allow(clippy::too_many_arguments)]
fn resolve_combats(
    mut ticks: EventReader<HourTick>,
    mut commands: Commands,
    mut combats: Query<(Entity, &mut ActiveCombat)>,
    mut divisions: Query<(Entity, &mut Division)>,
    mut selected: ResMut<SelectedDivision>,
    adjacency: Res<ProvinceAdjacency>,
    map: Res<ProvinceMap>,
    rivers: Res<ProvinceRivers>,
    air: Res<StrategicAirRegions>,
    mut experience: ResMut<ArmyExperience>,
    mut controllers: ResMut<ProvinceControllers>,
    mut controller_changed: EventWriter<ProvinceControllerChanged>,
) {
    let mut finished: HashSet<Entity> = HashSet::new();
    // Дивизии, на которые уже отдан despawn в этом вызове. `commands` применяются позже, поэтому
    // запросы ещё видят такие сущности — без этого набора их можно снести (и обработать) повторно.
    let mut despawned: HashSet<Entity> = HashSet::new();

    for _ in ticks.read() {
        // Детерминированный порядок: по провинции и владельцу атаки
        let mut list: Vec<(u32, String, Entity)> = combats
            .iter()
            .filter(|(ce, _)| !finished.contains(ce))
            .map(|(ce, c)| (c.province_id, c.attacker_owner.clone(), ce))
            .collect();
        list.sort_unstable();

        for (_, _, ce) in list {
            if finished.contains(&ce) {
                continue;
            }
            let Ok((_, existing)) = combats.get(ce) else { continue };
            let mut state = existing.clone();
            let province = state.province_id;

            // 0. Исчезнувшие дивизии (капитуляция, котёл) выбывают из боя
            state.attackers.retain(|e| !despawned.contains(e) && divisions.get(*e).is_ok());
            state.defenders.retain(|e| !despawned.contains(e) && divisions.get(*e).is_ok());

            // 1. Весь гарнизон провинции (стоящие дивизии защитника, не занятые другими боями) вступает в бой
            let engaged: HashSet<Entity> = combats
                .iter()
                .filter(|(other, _)| *other != ce)
                .flat_map(|(_, c)| c.attackers.iter().chain(c.defenders.iter()).copied())
                .collect();
            let mut joiners: Vec<(u32, Entity)> = divisions
                .iter()
                .filter(|(e, d)| {
                    !despawned.contains(e)
                        && d.owner == state.defender_owner
                        && d.current_province == province
                        && d.movement.is_none()
                        && !engaged.contains(e)
                        && !state.defenders.contains(e)
                        && !state.attackers.contains(e)
                })
                .map(|(e, d)| (d.id, e))
                .collect();
            joiners.sort_unstable();
            state.defenders.extend(joiners.into_iter().map(|(_, e)| e));

            // Одна из сторон исчезла — бой снимается, оставшиеся атакующие повторят попытку на границе
            if state.attackers.is_empty() || state.defenders.is_empty() {
                for &e in state.attackers.iter().chain(state.defenders.iter()) {
                    set_combat_state(&mut divisions, e, CombatState::None);
                }
                commands.entity(ce).despawn();
                finished.insert(ce);
                continue;
            }

            // 2. Лимит ширины: база + бонус за каждое уникальное направление атаки сверх первого
            let directions: BTreeSet<u32> = state
                .attackers
                .iter()
                .filter_map(|&e| divisions.get(e).ok().and_then(|(_, d)| d.movement.map(|m| m.from)))
                .collect();
            state.max_width = BASE_COMBAT_WIDTH + FLANK_WIDTH_BONUS * (directions.len() as u32).saturating_sub(1);

            // 3. Новые участники: на линию, пока вмещаются, иначе в резерв
            for side in [state.attackers.clone(), state.defenders.clone()] {
                let info = side_info(&divisions, &side);
                let mut used: u32 =
                    info.iter().filter(|i| i.3 == CombatState::Active).map(|i| i.2).sum();
                for &(_, e, width, st) in &info {
                    if st != CombatState::None {
                        continue;
                    }
                    let fits = used == 0 || used + width <= state.effective_max_width();
                    if fits {
                        used += width;
                    }
                    set_combat_state(&mut divisions, e, if fits { CombatState::Active } else { CombatState::Reserve });
                }
            }

            // 3б. Сужение фронта (фаза отхода): лишние дивизии линии с конца по id уходят в резерв
            let line_limit = state.effective_max_width();
            for side in [state.attackers.clone(), state.defenders.clone()] {
                let mut line: Vec<(u32, Entity, u32)> = side_info(&divisions, &side)
                    .into_iter()
                    .filter(|i| i.3 == CombatState::Active)
                    .map(|(id, e, width, _)| (id, e, width))
                    .collect();
                let mut total: u32 = line.iter().map(|l| l.2).sum();
                while total > line_limit && line.len() > 1 {
                    if let Some((id, e, width)) = line.pop() {
                        total -= width;
                        set_combat_state(&mut divisions, e, CombatState::Reserve);
                        println!("[БОЙ] Фронт сужен: дивизия #{id} отведена в резерв (провинция {province})");
                    }
                }
            }

            // 4. Урон: дивизии линии бьют по кругу (k-я атака -> k mod N-я дивизия линии противника)
            let snapshot = |divisions: &Query<(Entity, &mut Division)>, side: &[Entity]| -> Vec<(Entity, Division)> {
                side_info(divisions, side)
                    .into_iter()
                    .filter(|i| i.3 == CombatState::Active)
                    .filter_map(|(_, e, ..)| divisions.get(e).ok().map(|(_, d)| (e, d.clone())))
                    .collect()
            };
            let att_line = snapshot(&divisions, &state.attackers);
            let def_line = snapshot(&divisions, &state.defenders);

            // Разведка: сторона с лучшей средней разведкой на линии получает тактическое преимущество
            // (+5% к урону, вдвое выше шанс выхода резерва)
            let recon_adv = recon_advantage(
                &att_line.iter().map(|(_, d)| d.recon).collect::<Vec<_>>(),
                &def_line.iter().map(|(_, d)| d.recon).collect::<Vec<_>>(),
            );
            // Небо над стейтом провинции: перевес даёт до +10% к атаке, давление врага — до -15% к защите
            let att_air = air.edge(province, &state.attacker_owner, &state.defender_owner);
            let def_air = -att_air;
            let advantage = |damage: u32, has_advantage: bool| {
                if has_advantage { damage * (100 + TACTICAL_ADVANTAGE_PCT) / 100 } else { damage }
            };
            // Тактики: парированная сторона теряет бонусы, парирующая бьёт на 25% сильнее, отход режет урон
            let att_mods = side_mods(state.attacker_tactic, state.defender_tactic, true);
            let def_mods = side_mods(state.attacker_tactic, state.defender_tactic, false);
            // Ближний бой: броня не помогает, обе стороны бьют сильнее
            let point_blank = state.phase == CombatPhase::CloseQuarters;
            let cq_damage = |value: u32| if point_blank { value * CLOSE_QUARTERS_DAMAGE_PCT / 100 } else { value };

            if !att_line.is_empty() && !def_line.is_empty() {
                // Час боя на линии: каждые 24 дивизионо-часа дают стране +1 опыта (резерв не воюет и не считается)
                experience.record_combat_hour(&state.attacker_owner, att_line.len() as u32);
                experience.record_combat_hour(&state.defender_owner, def_line.len() as u32);
                // Каждая дивизия на линии набирает собственный боевой опыт (резерв не воюет)
                for (line_entity, _) in att_line.iter().chain(def_line.iter()) {
                    if let Ok((_, mut fighter)) = divisions.get_mut(*line_entity) {
                        fighter.gain_combat_experience_hour();
                    }
                }
                // Все удары считаются по значениям до боя, затем применяются
                let mut hits: Vec<(Entity, u32, u32)> = Vec::new();
                let terrain = terrain_modifiers(map.terrain_of(province));
                for (k, (_, striker)) in att_line.iter().enumerate() {
                    let (target_e, target) = &def_line[k % def_line.len()];
                    // Река между провинцией отправления этой дивизии и обороняемой — форсирование
                    // Захваченный мост снимает речной штраф форсирования
                    let over_river =
                        !state.bridge_held && striker.movement.is_some_and(|m| rivers.crosses(m.from, province));
                    let ctx = HitContext::for_attack(&terrain, striker, over_river)
                        .with_air(air_attack_bonus_pct(att_air), air_defense_penalty_pct(def_air))
                        .with_tactics(att_mods.attack_pct, def_mods.defense_pct)
                        .with_entrenchment_divisor(if state.phase == CombatPhase::Breakthrough { 2 } else { 1 });
                    // Наступающая дивизия получает бонус планирования к атаке
                    let bp = combat_bp_ctx(striker, target, striker.planning, ctx);
                    // Броня меняет урон и по организации, и по прочности; разведка добавляет +5%
                    let org_milli = apply_armor_rule(target.max_org * bp / 10, striker, target, point_blank).max(1);
                    let strength = apply_armor_rule(target.max_strength * bp / 50_000, striker, target, point_blank).max(1);
                    let tactical = recon_adv == Some(true);
                    let (org_milli, strength) = (
                        cq_damage(org_milli * att_mods.damage_pct / 100),
                        cq_damage(strength * att_mods.damage_pct / 100),
                    );
                    // Арьергардный бой: по организации обороняющихся урон ещё на четверть ниже
                    let org_milli = if state.phase == CombatPhase::TacticalWithdrawal {
                        org_milli * WITHDRAWAL_ORG_DAMAGE_PCT / 100
                    } else {
                        org_milli
                    };
                    hits.push((*target_e, advantage(org_milli, tactical), advantage(strength, tactical)));
                }
                for (k, (_, striker)) in def_line.iter().enumerate() {
                    let (target_e, target) = &att_line[k % att_line.len()];
                    // обороняющиеся бонуса планирования и рельефа не имеют, небо действует на обе стороны
                    let ctx = HitContext::NONE
                        .with_air(air_attack_bonus_pct(def_air), air_defense_penalty_pct(att_air))
                        .with_tactics(def_mods.attack_pct, att_mods.defense_pct);
                    let bp = combat_bp_ctx(striker, target, 0, ctx);
                    // Броня меняет урон и по организации, и по прочности; разведка добавляет +5%
                    let org_milli = apply_armor_rule(target.max_org * bp / 10, striker, target, point_blank).max(1);
                    let strength = apply_armor_rule(target.max_strength * bp / 50_000, striker, target, point_blank).max(1);
                    let tactical = recon_adv == Some(false);
                    let (org_milli, strength) = (
                        cq_damage(org_milli * def_mods.damage_pct / 100),
                        cq_damage(strength * def_mods.damage_pct / 100),
                    );
                    hits.push((*target_e, advantage(org_milli, tactical), advantage(strength, tactical)));
                }
                for (target, org_milli, strength) in hits {
                    if let Ok((_, mut t)) = divisions.get_mut(target) {
                        t.lose_org_milli(org_milli);
                        t.lose_strength(strength);
                    }
                }
            }

            // Прогресс: доля потерянной организации всех защитников (линия и резерв)
            let (mut total_max, mut total_cur) = (0u64, 0u64);
            for &e in &state.defenders {
                if let Ok((_, d)) = divisions.get(e) {
                    total_max += u64::from(d.max_org) * 1000;
                    total_cur += u64::from(d.org_milli());
                }
            }
            state.combat_progress =
                if total_max == 0 { 0 } else { ((total_max - total_cur.min(total_max)) * 100 / total_max) as u32 };
            state.elapsed_hours += 1;
            // Таймер фазы идёт по часовому тику
            if state.phase != CombatPhase::Default {
                state.phase_hours_left = state.phase_hours_left.saturating_sub(1);
            }

            // 5. Дивизии линии с нулевой организацией выходят из боя и освобождают ширину
            for attacker_side in [true, false] {
                let list = if attacker_side { state.attackers.clone() } else { state.defenders.clone() };
                for (id, e, _, st) in side_info(&divisions, &list) {
                    let broken = st == CombatState::Active
                        && divisions.get(e).is_ok_and(|(_, d)| d.current_org == 0);
                    if !broken {
                        continue;
                    }
                    set_combat_state(&mut divisions, e, CombatState::None);
                    if attacker_side {
                        state.attackers.retain(|&x| x != e);
                        if let Ok((_, mut a)) = divisions.get_mut(e) {
                            a.movement = None; // откат в провинцию отправления
                            a.path.clear();
                        }
                        println!("[БОЙ] Дивизия #{id} выбита с линии атаки на провинцию {province}");
                    } else {
                        state.defenders.retain(|&x| x != e);
                        let occupancy = occupancy_snapshot(&divisions, &despawned);
                        let candidates = safe_retreat_provinces(
                            province, &state.defender_owner, &adjacency, &map, &controllers, &occupancy,
                        );
                        retreat_or_capitulate(
                            &[(id, e)],
                            province,
                            &candidates,
                            u64::from(state.elapsed_hours),
                            &mut commands,
                            &mut divisions,
                            &mut despawned,
                            &mut selected,
                        );
                    }
                }
            }

            // 6. Резерв: детерминированный бросок 2..=5% в час на освободившуюся ширину
            for (is_attacker, side) in [(true, state.attackers.clone()), (false, state.defenders.clone())] {
                let info = side_info(&divisions, &side);
                let mut used: u32 =
                    info.iter().filter(|i| i.3 == CombatState::Active).map(|i| i.2).sum();
                for &(id, e, width, st) in &info {
                    if st != CombatState::Reserve || used + width > state.effective_max_width() {
                        continue;
                    }
                    // Сторона с лучшей разведкой быстрее реагирует на бреши в линии: шанс вдвое выше
                    let chance = reserve_chance_pct(id, recon_adv == Some(is_attacker));
                    let roll = det_rand(province as u64, id as u64, u64::from(state.elapsed_hours)) % 100;
                    if roll < chance {
                        used += width;
                        set_combat_state(&mut divisions, e, CombatState::Active);
                        println!("[БОЙ] Дивизия #{id} из резерва вступила в бой за провинцию {province}");
                    }
                }
            }

            // 7. Итог: сторона без дивизий на линии проигрывает
            let line_count = |divisions: &Query<(Entity, &mut Division)>, side: &[Entity]| {
                side_info(divisions, side).iter().filter(|i| i.3 == CombatState::Active).count()
            };
            let (att_active, def_active) =
                (line_count(&divisions, &state.attackers), line_count(&divisions, &state.defenders));
            let outcome = match (att_active > 0, def_active > 0) {
                (true, true) => Outcome::Continue,
                (true, false) => Outcome::AttackerWins,
                (false, true) => Outcome::AttackerLoses,
                (false, false) => {
                    // Обе линии выбиты одновременно: у кого осталось больше прочности, при равенстве — жребий по хэшу
                    let strength = |side: &[Entity]| -> u32 {
                        side.iter().filter_map(|&e| divisions.get(e).ok()).map(|(_, d)| d.current_strength).sum()
                    };
                    let (a, d) = (strength(&state.attackers), strength(&state.defenders));
                    if a != d {
                        if a > d { Outcome::AttackerWins } else { Outcome::AttackerLoses }
                    } else if det_rand(province as u64, u64::from(state.elapsed_hours), 0x7137) & 1 == 0 {
                        Outcome::AttackerWins
                    } else {
                        Outcome::AttackerLoses
                    }
                }
            };

            match outcome {
                Outcome::Continue => {
                    if let Ok((_, mut c)) = combats.get_mut(ce) {
                        *c = state;
                    }
                }
                Outcome::AttackerLoses => {
                    // Атака отбита: все атакующие откатываются в провинции отправления
                    for &e in &state.attackers {
                        if let Ok((_, mut a)) = divisions.get_mut(e) {
                            a.movement = None;
                            a.path.clear();
                            a.combat_state = CombatState::None;
                        }
                    }
                    for &e in &state.defenders {
                        set_combat_state(&mut divisions, e, CombatState::None);
                    }
                    println!("[БОЙ] Атака на провинцию {province} отбита");
                    commands.entity(ce).despawn();
                    finished.insert(ce);
                }
                Outcome::AttackerWins => {
                    // Оборона прорвана: отступает (или гибнет в котле) весь гарнизон, включая резерв
                    let mut retreating: Vec<(u32, Entity)> = divisions
                        .iter()
                        .filter(|(e, d)| {
                            !despawned.contains(e)
                                && d.owner == state.defender_owner
                                && d.current_province == province
                                && d.movement.is_none()
                        })
                        .map(|(e, d)| (d.id, e))
                        .collect();
                    retreating.sort_unstable();
                    let occupancy = occupancy_snapshot(&divisions, &despawned);
                    let candidates = safe_retreat_provinces(
                        province, &state.defender_owner, &adjacency, &map, &controllers, &occupancy,
                    );
                    retreat_or_capitulate(
                        &retreating,
                        province,
                        &candidates,
                        u64::from(state.elapsed_hours),
                        &mut commands,
                        &mut divisions,
                        &mut despawned,
                        &mut selected,
                    );
                    for &e in &state.defenders {
                        set_combat_state(&mut divisions, e, CombatState::None);
                    }

                    // Все атакующие (линия и резерв) занимают провинцию
                    for &e in &state.attackers {
                        if let Ok((_, mut a)) = divisions.get_mut(e) {
                            a.current_province = province;
                            a.movement = None;
                            a.combat_state = CombatState::None;
                        }
                    }
                    println!("[БОЙ] Провинцию {province} заняли дивизий: {}", state.attackers.len());

                    // Контроль переходит к нападающему на этом же часовом тике
                    if controllers.0.get(&province) != Some(&state.attacker_owner) {
                        controllers.0.insert(province, state.attacker_owner.clone());
                        println!("[ОККУПАЦИЯ] Провинция #{province} перешла под контроль {}", state.attacker_owner);
                        controller_changed.send(ProvinceControllerChanged {
                            province_id: province,
                            new_controller: state.attacker_owner.clone(),
                        });
                    }
                    commands.entity(ce).despawn();
                    finished.insert(ce);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Визуал (f32 только здесь, на симуляцию не влияет)
// ---------------------------------------------------------------------------

/// Цель отрисовки: центр провинции или точка на отрезке между центрами при марше.
/// Дивизия, упёршаяся в бой (`hours_left == 0`), рисуется на 70% пути.
fn update_division_positions(
    time: Res<Time>,
    centers: Res<ProvinceCenters>,
    mut query: Query<(&Division, &mut Transform)>,
) {
    let smooth = (VISUAL_SMOOTHING * time.delta_seconds()).min(1.0);

    for (division, mut transform) in &mut query {
        let target = match division.movement {
            Some(m) => {
                let (Some(a), Some(b)) = (centers.centers.get(&m.from), centers.centers.get(&m.to)) else {
                    continue;
                };
                let done = if m.hours_left == 0 {
                    0.7
                } else {
                    (m.total_hours - m.hours_left) as f32 / m.total_hours.max(1) as f32
                };
                a.lerp(*b, done)
            }
            // Стоящие дивизии провинции сходятся в её центр: на экране видна одна фишка стека
            None => match centers.centers.get(&division.current_province) {
                Some(c) => *c,
                None => continue,
            },
        };
        let current = transform.translation.truncate();
        let next = current.lerp(target, smooth);
        transform.translation = next.extend(DIVISION_Z);
    }
}

/// Billboard: масштаб = scale ортографической проекции, поэтому размер на экране постоянен.
fn scale_divisions_to_zoom(
    cameras: Query<&OrthographicProjection, With<MainCamera>>,
    mut query: Query<&mut Transform, With<Division>>,
) {
    let Ok(projection) = cameras.get_single() else { return };
    let scale = Vec3::new(projection.scale, projection.scale, 1.0);
    for mut transform in &mut query {
        if transform.scale != scale {
            transform.scale = scale;
        }
    }
}

/// Агрегация стеков (только визуал): в провинции видна одна фишка стоящего стека, остальные скрыты.
/// Полоски организации и прочности на видимой фишке — среднее арифметическое процентов всех дивизий
/// стека. Марширующие дивизии рисуются отдельно, со своими значениями.
fn update_stack_visuals(
    selected: Res<SelectedDivision>,
    mut view: ResMut<StackView>,
    mut divisions: Query<(Entity, &Division, &mut Visibility)>,
    mut bars: Query<(&Parent, &mut Sprite, Has<OrgBar>), (Or<(With<OrgBar>, With<StrengthBar>)>, Without<Division>)>,
) {
    view.0 = stack_layout(
        divisions.iter().filter(|(_, d, _)| d.movement.is_none()).map(|(e, d, _)| (e, d.id, d.current_province)),
        selected.0,
    );
    // Значения полосок для главных фишек стеков: среднее по членам
    let mut averages: BTreeMap<Entity, (u32, u32)> = BTreeMap::new();
    for stack in view.0.values() {
        let (mut orgs, mut hps) = (Vec::new(), Vec::new());
        for &(_, e) in &stack.members {
            if let Ok((_, d, _)) = divisions.get(e) {
                orgs.push(percent(d.current_org, d.max_org));
                hps.push(percent(d.current_strength, d.max_strength));
            }
        }
        averages.insert(stack.head, (average_percent(&orgs), average_percent(&hps)));
    }

    let heads: BTreeSet<Entity> = view.0.values().map(|s| s.head).collect();
    for (entity, d, mut visibility) in &mut divisions {
        let shown = d.movement.is_some() || heads.contains(&entity);
        let wanted = if shown { Visibility::Inherited } else { Visibility::Hidden };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
    for (parent, mut sprite, is_org) in &mut bars {
        let Ok((_, d, _)) = divisions.get(parent.get()) else { continue };
        let (org, hp) = averages
            .get(&parent.get())
            .copied()
            .unwrap_or((percent(d.current_org, d.max_org), percent(d.current_strength, d.max_strength)));
        let value = if is_org { org } else { hp };
        let size = Some(Vec2::new(crate::ui::micro_bar_width(value, BAR_MAX_WIDTH), BAR_HEIGHT));
        if sprite.custom_size != size {
            sprite.custom_size = size;
        }
    }
}

/// Значок `[N]` в углу фишки стека (рисуется egui под окнами, мимо мыши).
fn stack_badges_ui(
    mut contexts: EguiContexts,
    view: Res<StackView>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    transforms: Query<&Transform, With<Division>>,
) {
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    let Ok((camera, camera_transform)) = cameras.get_single() else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("stack_badges")));
    for stack in view.0.values().filter(|s| s.members.len() > 1) {
        let Ok(transform) = transforms.get(stack.head) else { continue };
        let Some(screen) = camera.world_to_viewport(camera_transform, transform.translation) else { continue };
        // правый верхний угол фишки: размер на экране постоянен (billboard)
        let anchor = egui::pos2(screen.x + DIVISION_SIZE.x * 0.5, screen.y - DIVISION_SIZE.y * 0.5);
        let text = stack_badge_text(stack.members.len());
        let rect = egui::Rect::from_min_size(anchor + egui::vec2(-2.0, -8.0), egui::vec2(8.0 * text.len() as f32 + 4.0, 13.0));
        painter.rect_filled(rect, 3.0, egui::Color32::from_rgb(20, 25, 35));
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(11.0), egui::Color32::from_rgb(255, 220, 90));
    }
}

/// Рамка выбранной дивизии подсвечивается жёлтым.
fn update_selection_visual(selected: Res<SelectedDivision>, mut query: Query<(Entity, &mut Sprite), With<Division>>) {
    for (entity, mut sprite) in &mut query {
        let color = if selected.0 == Some(entity) { SELECTED_FRAME_COLOR } else { FRAME_COLOR };
        if sprite.color != color {
            sprite.color = color;
        }
    }
}

// ---------------------------------------------------------------------------
// UI (bevy_egui)
// ---------------------------------------------------------------------------

/// Сводка по стороне боя для интерфейса.
struct SideStats {
    /// Занятая ширина фронта (дивизии на линии).
    used_width: u32,
    /// Дивизий в резерве.
    reserves: u32,
    /// Организация дивизий на линии: сумма текущей / сумма максимальной.
    org: u32,
    org_max: u32,
    /// Прочность (укомплектованность) дивизий на линии: сумма текущей / сумма максимальной.
    strength: u32,
    strength_max: u32,
    /// Сумма разведки и число дивизий на линии — для средней разведки стороны.
    recon_sum: u32,
    active: u32,
}

fn side_stats(divisions: &Query<&Division>, list: &[Entity]) -> SideStats {
    let mut stats = SideStats { used_width: 0, reserves: 0, org: 0, org_max: 0, strength: 0, strength_max: 0, recon_sum: 0, active: 0 };
    for d in list.iter().filter_map(|&e| divisions.get(e).ok()) {
        match d.combat_state {
            CombatState::Active => {
                stats.used_width += d.combat_width;
                stats.org += d.current_org;
                stats.org_max += d.max_org;
                stats.strength += d.current_strength;
                stats.strength_max += d.max_strength;
                stats.recon_sum += d.recon;
                stats.active += 1;
            }
            CombatState::Reserve => stats.reserves += 1,
            CombatState::None => {}
        }
    }
    stats
}

/// Средняя разведка стороны с одним знаком после запятой, без чисел с плавающей точкой: `2.5`.
fn recon_text(recon_sum: u32, active: u32) -> String {
    if active == 0 {
        return "0.0".to_string();
    }
    let tenths = recon_sum * 10 / active;
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// Статус укомплектованности техникой и оружием: `Комплектность: Винтовки 900/900 | Пушки 24/24`;
/// типы, которых не хватило при последнем пополнении, перечисляются как дефицит.
fn equipment_status_text(d: &Division) -> String {
    let parts: Vec<String> =
        d.equipment_needs.iter().map(|(&t, &need)| format!("{} {}/{}", t.label(), d.equipment_of(t), need)).collect();
    let mut text = format!("Комплектность: {}", if parts.is_empty() { "не требуется".to_string() } else { parts.join(" | ") });
    if !d.equipment_shortage.is_empty() {
        let lacking: Vec<&str> = d.equipment_shortage.iter().map(|t| t.label()).collect();
        text.push_str(&format!(" | Дефицит: {}", lacking.join(", ")));
    }
    text
}

/// Строка атаки в панели дивизии: `Атака: 90 мягк. / 0 твёрд. | Твёрдость: 0%`.
fn attack_label(d: &Division) -> String {
    format!("Атака: {} мягк. / {} твёрд. | Твёрдость: {}%", d.soft_attack, d.hard_attack, d.hardness)
}

/// Строка разведки и неба в окне боя: `Разведка: 3.0 vs 1.0 | Небо: Германия +40`.
fn recon_air_label(att: &SideStats, def: &SideStats, air_line: &str) -> String {
    format!(
        "Разведка: {} vs {} | {air_line}",
        recon_text(att.recon_sum, att.active),
        recon_text(def.recon_sum, def.active),
    )
}

/// Строка модификаторов боя: `Местность: forest (Защ: +20%, Атк: -10%)`.
/// В штраф атаки входит форсирование реки (`river_penalty`, обычно -30%, с сапёрами -10%),
/// о котором напоминает пометка `[река]`.
fn terrain_label(terrain: Option<&str>, river_penalty: Option<u32>) -> String {
    let mods = terrain_modifiers(terrain);
    let river_crossing = river_penalty.is_some();
    let river = river_penalty.unwrap_or(0);
    format!(
        "Местность: {} (Защ: +{}%, Атк: -{}%){}",
        terrain.unwrap_or("unknown"),
        mods.defense_bonus,
        mods.attack_penalty + river,
        if river_crossing { " [река]" } else { "" }
    )
}

/// Бои, у которых раскрыта подробная плашка (клик по боевому баблу). Только отображение.
#[derive(Resource, Default)]
struct OpenCombats(BTreeSet<Entity>);

/// Маркер боя над провинцией: круглый бейдж с процентом, организация линии, ширина фронта и резерв.
fn combat_ui(
    mut contexts: EguiContexts,
    combats: Query<(Entity, &ActiveCombat)>,
    divisions: Query<&Division>,
    centers: Res<ProvinceCenters>,
    map: Res<ProvinceMap>,
    rivers: Res<ProvinceRivers>,
    air: Res<StrategicAirRegions>,
    colors: Res<CountryColors>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut open: ResMut<OpenCombats>,
) {
    let Ok((camera, cam_tf)) = cameras.get_single() else { return };
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    open.0.retain(|&e| combats.contains(e));

    for (ce, combat) in &combats {
        let Some(center) = centers.centers.get(&combat.province_id) else { continue };
        let Some(screen) = camera.world_to_viewport(cam_tf, center.extend(0.0)) else { continue };
        let att = side_stats(&divisions, &combat.attackers);
        let def = side_stats(&divisions, &combat.defenders);
        // Боевой бабл над фишками обороняемой провинции: размер в пикселях экрана постоянен при любом зуме
        let progress = crate::ui::bubble_progress(percent(att.org, att.org_max), percent(def.org, def.org_max));
        let radius = crate::ui::BUBBLE_RADIUS;
        let mut toggled = false;
        egui::Area::new(egui::Id::new(("combat_bubble", ce)))
            .fixed_pos(egui::pos2(screen.x - radius, screen.y - DIVISION_SIZE.y - radius * 2.0 - 8.0))
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(egui::vec2(radius * 2.0, radius * 2.0), egui::Sense::click());
                let painter = ui.painter();
                painter.circle_filled(rect.center(), radius, crate::ui::bubble_color(progress));
                painter.circle_stroke(rect.center(), radius, egui::Stroke::new(2.5_f32, crate::ui::BUBBLE_RIM));
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    progress.to_string(),
                    egui::FontId::proportional(14.0),
                    egui::Color32::WHITE,
                );
                toggled = response.on_hover_text("Подробности боя").clicked();
            });
        if toggled && !open.0.remove(&ce) {
            open.0.insert(ce);
        }
        if !open.0.contains(&ce) {
            continue;
        }
        // Рельеф обороняемой провинции и форсирование реки хотя бы одной атакующей дивизией
        // (в плашке — самый тяжёлый штраф среди форсирующих: сапёры его снижают)
        let river_penalty = combat
            .attackers
            .iter()
            .filter_map(|&e| divisions.get(e).ok())
            .filter(|d| d.movement.is_some_and(|m| rivers.crosses(m.from, combat.province_id)))
            .map(river_penalty_for)
            .max();
        let attacker_color = colors.ui_color32(&combat.attacker_owner);
        let defender_color = colors.ui_color32(&combat.defender_owner);
        let terrain_text = terrain_label(map.terrain_of(combat.province_id), river_penalty);
        let edge = air.edge(combat.province_id, &combat.attacker_owner, &combat.defender_owner);
        let sky = air_line(air.region_name(combat.province_id), edge, &combat.attacker_owner, &combat.defender_owner);
        let recon_text = recon_air_label(&att, &def, &sky);

        egui::Area::new(egui::Id::new(("combat_plate", ce)))
            .fixed_pos(egui::pos2(screen.x + radius + 6.0, screen.y - DIVISION_SIZE.y - radius * 2.0 - 8.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(egui::RichText::new(format!("Прогресс боя: {progress}% | Потери обороны: {}%", combat.combat_progress)).small());
                ui.label(egui::RichText::new(&terrain_text).small());
                ui.label(egui::RichText::new(&recon_text).small());
                show_tactics(ui, combat);
                // Организация линии: атакующие (синий) и защитники (красный), ширина фронта и резерв
                let sides = [
                    ("АТК", &att, egui::Color32::from_rgb(70, 160, 220), attacker_color),
                    ("ЗАЩ", &def, egui::Color32::from_rgb(220, 90, 70), defender_color),
                ];
                for (tag, side, color, country_color) in sides {
                    let _ = color;
                    crate::ui::status_bar(ui, "Организация", side.org, side.org_max, crate::ui::ORG_COLOR, 160.0);
                    crate::ui::status_bar(ui, "Прочность", side.strength, side.strength_max, crate::ui::STRENGTH_COLOR, 160.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{tag} Ширина фронта: {}/{} | В резерве: {} див.",
                            side.used_width, combat.max_width, side.reserves
                        ))
                        .color(country_color)
                        .small(),
                    );
                }
            });
    }
}

/// Окно выбранной дивизии: организация, прочность и статус.
fn division_info_ui(
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedDivision>,
    mut group: ResMut<SelectedGroup>,
    mut multi: ResMut<SelectedDivisions>,
    divisions: Query<&Division>,
    all: Query<(Entity, &Division)>,
    combats: Query<&ActiveCombat>,
    map: Res<ProvinceMap>,
    templates: Res<DivisionTemplates>,
    colors: Res<CountryColors>,
    mut flags: ResMut<crate::flags::FlagTextures>,
    names: Res<crate::ui::CountryNames>,
    mut refit_out: EventWriter<RefitDivision>,
) {
    let Some(entity) = selected.0 else { return };
    let Ok(d) = divisions.get(entity) else { return };
    let Some(ctx) = contexts.try_ctx_mut() else { return };

    // Строки стека: стоящие дивизии той же провинции по id (для марширующей дивизии список пуст)
    let rows: Vec<(Entity, String)> = if d.movement.is_none() {
        let mut standing: Vec<(u32, Entity, &Division)> = all
            .iter()
            .filter(|(_, o)| o.current_province == d.current_province && o.movement.is_none())
            .map(|(e, o)| (o.id, e, o))
            .collect();
        standing.sort_unstable_by_key(|&(id, _, _)| id);
        standing.iter().enumerate().map(|(i, &(_, e, o))| (e, stack_row_text(i + 1, o))).collect()
    } else {
        Vec::new()
    };
    let mut clicked: Option<Entity> = None;
    // Выделение рамкой: строки всех живых дивизий группы
    let multi_rows: Vec<(Entity, String)> = multi
        .0
        .iter()
        .filter_map(|&e| all.get(e).ok())
        .enumerate()
        .map(|(i, (e, o))| (e, stack_row_text(i + 1, o)))
        .collect();
    let mut focus: Option<Entity> = None;
    let mut clear_group = false;

    let combat = combats.iter().find(|c| c.involves(entity));
    let status = match combat {
        Some(c) if d.combat_state == CombatState::Reserve => format!("В резерве боя ({}%)", c.combat_progress),
        Some(c) => format!("В бою ({}%)", c.combat_progress),
        None => match d.movement {
            Some(m) if m.hours_left > 0 => format!("На марше (осталось {} ч.)", m.hours_left),
            Some(_) => "На марше".to_string(), // дошла до границы, ждёт занятия провинции
            None => "В обороне".to_string(),
        },
    };
    let owner_color = colors.ui_color32(&d.owner);
    let frame = egui::Frame::window(&ctx.style())
        .fill(crate::ui::PANEL_FILL)
        .stroke(egui::Stroke::new(2.0_f32, owner_color));

    egui::Window::new(crate::ui::window_title("Дивизия"))
        .anchor(egui::Align2::LEFT_TOP, [10.0, 10.0])
        .resizable(false)
        .collapsible(false)
        .frame(frame)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                crate::ui::country_header(ui, &mut flags, &colors, &names, &d.owner, &format!("Дивизия \"{}\"", d.name));
                ui.label(nato_badge_text(d.nato_symbol));
            });
            crate::ui::readiness_bars(ui, (d.current_org, d.max_org), (d.current_strength, d.max_strength));
            // Стек провинции: кликабельные строки; ПКМ командует всей группой, пока она выбрана целиком
            if !multi_rows.is_empty() {
                ui.label(format!("Выделено рамкой: {} див. (ПКМ — приказ всем)", multi_rows.len()));
                for (row_entity, text) in &multi_rows {
                    if ui.selectable_label(*row_entity == entity, text).clicked() {
                        focus = Some(*row_entity);
                    }
                }
                if ui.button("Сбросить группу").clicked() {
                    clear_group = true;
                }
                ui.separator();
            } else if rows.len() > 1 {
                ui.label(format!(
                    "Стек в провинции #{} ({}). {}",
                    d.current_province,
                    stack_badge_text(rows.len()),
                    if group.whole { "Выбрана вся группа" } else { "Выбрана одна дивизия" }
                ));
                for (row_entity, text) in &rows {
                    if ui.selectable_label(*row_entity == entity && !group.whole, text).clicked() {
                        clicked = Some(*row_entity);
                    }
                }
                if ui.add_enabled(!group.whole, egui::Button::new("Выбрать всю группу")).clicked() {
                    group.whole = true;
                }
                ui.separator();
            }
            ui.label(format!("ID: {} | Стартовая оснащённость {}%", d.id, d.start_equipment_pct));
            ui.label(experience_text(d.experience_pct));
            ui.label(format!("Провинция: {}", d.current_province));
            ui.label(format!(
                "Укомплектованность: {}/{} (Потери: {})",
                d.current_strength, d.max_strength, d.losses
            ));
            ui.label(attack_label(d));
            ui.label(format!("Защита: {}", d.defense));
            ui.label(format!(
                "Окопанность: {}/{} (+{}% защиты)",
                d.entrenchment,
                d.max_entrenchment,
                d.entrenchment * 2
            ));
            if d.support.is_empty() {
                ui.label("Поддержка: нет");
            } else {
                ui.label(format!("Поддержка: [{}]", d.support.join(", ")));
            }
            ui.label(format!(
                "Шаблон: \"{}\" ({} батальонов) | Личный состав: {} чел. | Снаряжение: {}/{} шт.",
                d.template_name,
                d.battalion_count,
                d.current_manpower(),
                d.current_equipment(),
                d.required_equipment()
            ));
            ui.label(equipment_status_text(d));
            ui.label(format!("Состояние: {status}"));
            if let Some(c) = combat {
                show_tactics(ui, c);
            }
            ui.label(format!("Боевая ширина дивизии: {}", d.combat_width));
            // Смена шаблона: доступна нации, только пока дивизия стоит и не воюет
            ui.separator();
            ui.label("Сменить шаблон:");
            let can_refit = d.movement.is_none() && combat.is_none();
            ui.horizontal_wrapped(|ui| {
                for name in templates.available_names(&d.owner) {
                    if name == d.template_name {
                        continue;
                    }
                    if ui.add_enabled(can_refit, egui::Button::new(name)).clicked() {
                        refit_out.send(RefitDivision { division: entity, template: name.to_string() });
                    }
                }
            });
            ui.separator();
            ui.label(format!("Разведка: {}", d.recon));
            if d.is_supplied {
                ui.label("Снабжение: В норме");
            } else {
                ui.label(
                    egui::RichText::new(format!(
                        "Снабжение: Отрезано (Истощение {} дн., -33% к статам)",
                        d.attrition_days
                    ))
                    .color(egui::Color32::from_rgb(230, 120, 80)),
                );
            }
            ui.label(format!(
                "План наступления: {}/{MAX_PLANNING} (+{}% к атаке)",
                d.planning, d.planning
            ));
            ui.label(format!(
                "Скорость: {}.{} км/ч | Броня: {} | Пробитие: {}",
                d.max_speed / 10,
                d.max_speed % 10,
                d.armor,
                d.piercing
            ));
            if let Some(c) = combat {
                // Ширина фронта и резерв той стороны, за которую воюет дивизия
                let own_side = if c.attackers.contains(&entity) { &c.attackers } else { &c.defenders };
                let side = side_stats(&divisions, own_side);
                ui.label(format!(
                    "Ширина фронта: {}/{} | В резерве: {} див.",
                    side.used_width, c.max_width, side.reserves
                ));
            }
            if let Some(m) = d.movement {
                ui.label(format!(
                    "Шаг: {} -> {} ({}) | Осталось: {}/{} ч",
                    m.from,
                    m.to,
                    map.terrain_of(m.to).unwrap_or("unknown"),
                    m.hours_left,
                    m.total_hours
                ));
            }
            match (d.path.first(), d.path.last()) {
                (Some(_), Some(goal)) => {
                    let via = if d.path.len() > 1 { "... -> " } else { "" };
                    ui.label(format!(
                        "Маршрут: {} -> {via}{goal} (осталось шагов: {})",
                        d.current_province,
                        d.path.len()
                    ));
                }
                _ => {
                    ui.label("Маршрут: нет");
                }
            }
        });
    // выбор строки инспектора переключает управление на одну дивизию
    if let Some(row) = clicked {
        selected.0 = Some(row);
        group.whole = false;
    }
    // строка группы рамки только переводит фокус инспектора, приказ по-прежнему получает вся группа
    if let Some(row) = focus {
        selected.0 = Some(row);
    }
    if clear_group {
        multi.0.clear();
        group.whole = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_division(
        app: &mut App,
        id: u32,
        owner: &str,
        province: u32,
        movement: Option<MovementProgress>,
        attack_multiplier: u32,
    ) -> Entity {
        let templates = DivisionTemplates::default();
        let mut d = Division::from_template(
            id,
            format!("Div {id}"),
            owner.to_string(),
            province,
            BASE_TEMPLATE.to_string(),
            templates.get_or_base(BASE_TEMPLATE),
        );
        d.movement = movement;
        d.soft_attack *= attack_multiplier;
        app.world_mut().spawn(d).id()
    }

    /// Мини-карта: 1 (GER) – 2 (FRA, спорная) – 3 (FRA, тыл); 4 (GER) – второе направление атаки на 2.
    /// Атакуют `attackers` дивизий (id 1..), обороняют `defenders` дивизий (id 101..).
    /// `flank_from` — откуда идёт каждая атакующая дивизия.
    fn scenario(attackers: &[u32], defenders: u32, attack_multiplier: u32) -> (App, Vec<Entity>, Vec<Entity>) {
        let mut app = App::new();
        let neighbors = BTreeMap::from([
            (1, BTreeSet::from([2])),
            (2, BTreeSet::from([1, 3, 4])),
            (3, BTreeSet::from([2])),
            (4, BTreeSet::from([2])),
        ]);
        let controllers = ProvinceControllers(BTreeMap::from([
            (1, "GER".to_string()),
            (2, "FRA".to_string()),
            (3, "FRA".to_string()),
            (4, "GER".to_string()),
        ]));
        app.add_event::<HourTick>()
            .add_event::<ProvinceControllerChanged>()
            .init_resource::<SelectedDivision>()
            .insert_resource(ProvinceMap::empty_for_test())
            .init_resource::<ProvinceRivers>()
            .init_resource::<StateRegistry>()
            .init_resource::<StrategicAirRegions>()
            .init_resource::<ArmyExperience>()
            .insert_resource(ProvinceAdjacency { neighbors })
            .insert_resource(controllers)
            .add_systems(Update, resolve_combats);

        let att: Vec<Entity> = attackers
            .iter()
            .enumerate()
            .map(|(i, &from)| {
                let m = MovementProgress { from, to: 2, hours_left: 0, total_hours: 24 };
                spawn_division(&mut app, i as u32 + 1, "GER", from, Some(m), attack_multiplier)
            })
            .collect();
        let def: Vec<Entity> =
            (0..defenders).map(|i| spawn_division(&mut app, 101 + i, "FRA", 2, None, 1)).collect();

        app.world_mut().spawn(ActiveCombat {
            province_id: 2,
            attacker_owner: "GER".to_string(),
            defender_owner: "FRA".to_string(),
            attackers: att.clone(),
            defenders: def.clone(),
            max_width: BASE_COMBAT_WIDTH,
            combat_progress: 0,
            elapsed_hours: 0,
            attacker_tactic: CombatTactic::Attack,
            defender_tactic: CombatTactic::Defend,
            is_attacker_countered: false,
            is_defender_countered: false,
            tactic_change_day: 0,
            phase: CombatPhase::Default,
            phase_hours_left: 0,
            bridge_held: false,
        });
        (app, att, def)
    }

    fn tick(app: &mut App) {
        app.world_mut().send_event(HourTick);
        app.update();
    }

    fn state_counts(app: &mut App, list: &[Entity]) -> (usize, usize) {
        let world = app.world();
        let states: Vec<CombatState> = list.iter().filter_map(|&e| world.get::<Division>(e)).map(|d| d.combat_state).collect();
        (
            states.iter().filter(|&&s| s == CombatState::Active).count(),
            states.iter().filter(|&&s| s == CombatState::Reserve).count(),
        )
    }

    #[test]
    fn width_limit_splits_line_and_reserve() {
        // 6 атакующих и 5 защитников по 18 ширины против лимита 80: на линии по 4, остальные в резерве
        let (mut app, att, def) = scenario(&[1; 6], 5, 1);
        tick(&mut app);
        assert_eq!(state_counts(&mut app, &att), (4, 2));
        assert_eq!(state_counts(&mut app, &def), (4, 1));
        let combat = app.world_mut().query::<&ActiveCombat>().single(app.world()).clone();
        assert_eq!(combat.max_width, 80);
    }

    #[test]
    fn flank_direction_adds_width() {
        // Атака из двух разных провинций (1 и 4): лимит 80 + 40 = 120, все 6 + 5 дивизий помещаются на линию
        let (mut app, att, def) = scenario(&[1, 1, 1, 4, 4, 4], 5, 1);
        tick(&mut app);
        assert_eq!(state_counts(&mut app, &att), (6, 0));
        assert_eq!(state_counts(&mut app, &def), (5, 0));
        let combat = app.world_mut().query::<&ActiveCombat>().single(app.world()).clone();
        assert_eq!(combat.max_width, 120);
    }

    #[test]
    fn breakthrough_occupies_province_and_defenders_retreat() {
        // Сильные атакующие выбивают линию защиты: резерв не успевает, бой проигран обороной
        let (mut app, att, def) = scenario(&[1; 6], 5, 20);
        let mut ticks = 0;
        while app.world_mut().query::<&ActiveCombat>().iter(app.world()).count() > 0 {
            tick(&mut app);
            ticks += 1;
            assert!(ticks < 500, "бой не закончился");
        }
        let world = app.world();
        let controllers = world.resource::<ProvinceControllers>();
        assert_eq!(controllers.0.get(&2).map(String::as_str), Some("GER"));
        for &e in &att {
            let d = world.get::<Division>(e).expect("атакующие живы");
            assert_eq!((d.current_province, d.combat_state), (2, CombatState::None));
            assert!(d.movement.is_none());
        }
        for &e in &def {
            // защитники отступили в тыловую провинцию 3
            let d = world.get::<Division>(e).expect("защитники отступили, а не погибли");
            assert_eq!((d.current_province, d.combat_state), (3, CombatState::None));
        }
        println!("бой закончился за {ticks} ч.");
    }

    /// Суммарная организация и прочность дивизий.
    fn totals(app: &App, list: &[Entity]) -> (u32, u32) {
        list.iter()
            .filter_map(|&e| app.world().get::<Division>(e))
            .fold((0, 0), |(org, st), d| (org + d.org_milli(), st + d.current_strength))
    }

    #[test]
    fn armored_defenders_take_less_and_deal_more_in_combat() {
        let run = |armored: bool| {
            let (mut app, att, def) = scenario(&[1; 4], 4, 1);
            if armored {
                for &e in &def {
                    let mut d = app.world_mut().get_mut::<Division>(e).unwrap();
                    (d.armor, d.piercing) = (11, 16); // как у танковой дивизии: пехота (пробитие 4) не пробивает
                }
            }
            for _ in 0..5 {
                tick(&mut app);
            }
            (totals(&app, &att), totals(&app, &def))
        };
        let ((att_plain, _), (def_plain, _)) = run(false);
        let ((att_armored, _), (def_armored, _)) = run(true);
        assert!(def_armored > def_plain, "бронированные защитники теряют меньше организации");
        assert!(att_armored < att_plain, "атакующие теряют больше из-за непробиваемой брони");
    }

    #[test]
    fn planning_boosts_attacker_damage_only() {
        let run = |planning: u32| {
            let (mut app, att, def) = scenario(&[1; 4], 4, 1);
            for &e in &att {
                app.world_mut().get_mut::<Division>(e).unwrap().planning = planning;
            }
            for _ in 0..5 {
                tick(&mut app);
            }
            (totals(&app, &att).0, totals(&app, &def).0)
        };
        let (att_plain, def_plain) = run(0);
        let (att_planned, def_planned) = run(30);
        assert!(def_planned < def_plain, "подготовленные атакующие бьют сильнее");
        assert_eq!(att_planned, att_plain, "потери атакующих не зависят от их планирования");
    }

    #[test]
    fn commander_skills_and_limit_penalty_scale_combat_power() {
        let templates = DivisionTemplates::default();
        let make = |id| {
            Division::from_template(id, "d".into(), "GER".into(), 1, PLAIN_INFANTRY_TEMPLATE.to_string(), templates.get_or_base(PLAIN_INFANTRY_TEMPLATE)).trained()
        };
        let (mut attacker, mut defender) = (make(1), make(2));
        // база: атака 90, защита 225 -> 800 * 90 / 315 = 228
        assert_eq!(combat_bp(&attacker, &defender, 0), 228);
        // навык атаки 4 (+8%): 90 * 108 / 100 = 97 -> 800 * 97 / 322 = 240
        attacker.command_attack_skill = 4;
        assert_eq!(combat_bp(&attacker, &defender, 0), 240);
        // навык защиты 3 (+6%): 225 * 106 / 100 = 238 -> 800 * 97 / 335 = 231
        defender.command_defense_skill = 3;
        assert_eq!(combat_bp(&attacker, &defender, 0), 231);
        // сверх лимита командира: атака вдвое слабее (97 * 50 / 100 = 48 -> 800 * 48 / 286 = 134)
        attacker.over_command_limit = true;
        assert_eq!(combat_bp(&attacker, &defender, 0), 134);
        // и защита вдвое слабее (238 * 50 / 100 = 119 -> 800 * 48 / 167 = 229)
        defender.over_command_limit = true;
        assert_eq!(combat_bp(&attacker, &defender, 0), 229);
    }

    #[test]
    fn attack_skill_makes_attackers_hit_harder_in_combat() {
        let run = |skill: u32| {
            let (mut app, att, def) = scenario(&[1; 4], 4, 1);
            for &e in &att {
                app.world_mut().get_mut::<Division>(e).unwrap().command_attack_skill = skill;
            }
            for _ in 0..5 {
                tick(&mut app);
            }
            totals(&app, &def).0
        };
        assert!(run(4) < run(0), "командир с навыком атаки наносит больше урона");
    }
    #[test]
    fn unsupplied_division_loses_a_third_of_attack_and_defense() {
        let templates = DivisionTemplates::default();
        let make = |id| {
            Division::from_template(id, "d".into(), "GER".into(), 1, PLAIN_INFANTRY_TEMPLATE.to_string(), templates.get_or_base(PLAIN_INFANTRY_TEMPLATE)).trained()
        };
        let (mut attacker, mut defender) = (make(1), make(2));
        assert_eq!(combat_bp(&attacker, &defender, 0), 228);
        // отрезанный атакующий: атака 90 * 67 / 100 = 60 -> 800 * 60 / 285 = 168
        attacker.is_supplied = false;
        assert_eq!(combat_bp(&attacker, &defender, 0), 168);
        // отрезанный защитник: защита 225 * 67 / 100 = 150 -> 800 * 90 / 240 = 300
        attacker.is_supplied = true;
        defender.is_supplied = false;
        assert_eq!(combat_bp(&attacker, &defender, 0), 300);
    }

    #[test]
    fn cut_off_attackers_deal_less_damage_in_combat() {
        let run = |supplied: bool| {
            let (mut app, att, def) = scenario(&[1; 4], 4, 1);
            for &e in &att {
                app.world_mut().get_mut::<Division>(e).unwrap().is_supplied = supplied;
            }
            for _ in 0..5 {
                tick(&mut app);
            }
            totals(&app, &def).0
        };
        assert!(run(false) > run(true), "защитники теряют меньше организации, когда атакующие отрезаны");
    }
    #[test]
    fn terrain_and_river_protect_defenders_in_combat() {
        // org защитников после 5 часов боя: чем лучше оборона, тем меньше потери
        let run = |terrain: &str, river: bool| {
            let (mut app, _att, def) = scenario(&[1; 4], 4, 1);
            app.insert_resource(ProvinceMap::with_terrain_for_test(&[(2, terrain)]));
            if river {
                app.insert_resource(ProvinceRivers(BTreeSet::from([(1, 2)])));
            }
            for _ in 0..5 {
                tick(&mut app);
            }
            totals(&app, &def).0
        };
        let plains = run("plains", false);
        let forest = run("forest", false);
        let mountain = run("mountain", false);
        let mountain_river = run("mountain", true);
        assert!(forest > plains, "лес защищает: потери меньше, чем на равнине");
        assert!(mountain > forest, "горы защищают сильнее леса");
        assert!(mountain_river > mountain, "форсирование реки ослабляет атаку ещё сильнее");
    }
    #[test]
    fn better_recon_adds_five_percent_damage_for_that_side_only() {
        let run = |attacker_recon: u32| {
            let (mut app, att, def) = scenario(&[1; 4], 4, 1);
            for &e in &att {
                app.world_mut().get_mut::<Division>(e).unwrap().recon = attacker_recon;
            }
            for _ in 0..5 {
                tick(&mut app);
            }
            (totals(&app, &att).0, totals(&app, &def).0)
        };
        // защитники с разведкой 1: равенство (1 vs 1) и превосходство атакующих (3 vs 1)
        let (att_equal, def_equal) = run(1);
        let (att_better, def_better) = run(3);
        assert!(def_better < def_equal, "атакующие с лучшей разведкой бьют сильнее");
        assert_eq!(att_better, att_equal, "потери атакующих не зависят от их же разведки");
        // обратный случай: атакующие слабее в разведке — преимущество у защитников
        let (att_worse, _) = run(0);
        assert!(att_worse < att_equal, "защитники с лучшей разведкой бьют сильнее");
    }

    fn air_scenario(air_value: i32) -> (App, Vec<Entity>, Vec<Entity>) {
        let (mut app, att, def) = scenario(&[1; 4], 4, 1);
        // провинция 2 принадлежит стейту 7 с заданным небом (плюс — Германия)
        let state = std::sync::Arc::new(crate::map::StateDef {
            id: 7,
            name: "S7".into(),
            owner: "FRA".into(),
            provinces: vec![2],
            victory_points: vec![],
            civilian_factories: 0,
            military_factories: 0,
            resources: Default::default(),
        });
        let mut registry = StateRegistry::default();
        registry.province_to_state.insert(2, state);
        app.insert_resource(registry);
        // провинция 2 лежит в стратегическом регионе 7; положительное небо — авиация Германии, отрицательное — Франции
        let mut air = StrategicAirRegions::default();
        air.insert(crate::air::StrategicRegion { id: 7, name: "Западная Германия".into(), provinces: vec![2] });
        air.set_power(7, "GER", air_value.max(0));
        air.set_power(7, "FRA", (-air_value).max(0));
        app.insert_resource(air);
        (app, att, def)
    }

    #[test]
    fn air_superiority_helps_the_stronger_sky_side_in_combat() {
        let run = |air_value: i32| {
            let (mut app, att, def) = air_scenario(air_value);
            for _ in 0..5 {
                tick(&mut app);
            }
            (totals(&app, &att).0, totals(&app, &def).0)
        };
        let (att_neutral, def_neutral) = run(0);
        // небо за Германией (атакующие GER): защитники FRA теряют больше, атакующие — не больше
        let (att_german_sky, def_german_sky) = run(80);
        assert!(def_german_sky < def_neutral, "перевес в воздухе усиливает атаку и ослабляет защиту врага");
        assert!(att_german_sky >= att_neutral, "атакующим не хуже");
        // небо за Францией: теперь французские защитники бьют сильнее, германские теряют больше
        let (att_french_sky, def_french_sky) = run(-80);
        assert!(att_french_sky < att_neutral, "под вражеским небом атакующие теряют больше");
        assert!(def_french_sky >= def_neutral, "защитникам не хуже");
    }
    #[test]
    fn hardness_of_defenders_changes_how_hard_attackers_hit_in_combat() {
        // org защитников после 5 часов боя: атакующие с заданными soft/hard против защитников с твёрдостью
        let run = |soft: u32, hard: u32, defender_hardness: u32| {
            let (mut app, att, def) = scenario(&[1; 4], 4, 1);
            for &e in &att {
                let mut d = app.world_mut().get_mut::<Division>(e).unwrap();
                (d.soft_attack, d.hard_attack) = (soft, hard);
            }
            for &e in &def {
                app.world_mut().get_mut::<Division>(e).unwrap().hardness = defender_hardness;
            }
            for _ in 0..5 {
                tick(&mut app);
            }
            totals(&app, &def).0
        };
        let soft_vs_soft = run(90, 0, 0);
        let soft_vs_armor = run(90, 0, 100);
        let hard_vs_armor = run(0, 90, 100);
        let hard_vs_soft = run(0, 90, 0);
        assert!(soft_vs_armor > soft_vs_soft, "мягкая атака почти не вредит полностью бронированной цели");
        assert!(hard_vs_armor < soft_vs_armor, "жёсткая атака по технике сильнее мягкой");
        assert!(hard_vs_soft > soft_vs_soft, "жёсткая атака по живой силе слабее мягкой");
    }

    #[test]
    fn combat_hours_on_the_line_grant_army_experience() {
        let xp = |app: &App, country: &str| app.world().resource::<ArmyExperience>().get(country);
        // 4 против 4 на линии: 24 часа по 4 дивизии = 96 дивизионо-часов = +4 опыта каждой стороне
        let (mut app, _att, _def) = scenario(&[1; 4], 4, 1);
        assert_eq!((xp(&app, "GER"), xp(&app, "FRA")), (10, 10), "старт: по 10 XP");
        for _ in 0..23 {
            tick(&mut app);
        }
        assert_eq!((xp(&app, "GER"), xp(&app, "FRA")), (13, 13), "23 часа * 4 = 92 -> +3, остаток 20");
        tick(&mut app);
        assert_eq!((xp(&app, "GER"), xp(&app, "FRA")), (14, 14), "остаток переносится: 96 -> +4");
    }

    #[test]
    fn reserves_do_not_earn_experience() {
        // 6 атакующих и 5 защитников, но на линии по 4 (лимит ширины 80): опыт считают только линия
        let (mut app, _att, _def) = scenario(&[1; 6], 5, 1);
        for _ in 0..24 {
            tick(&mut app);
        }
        let xp = app.world().resource::<ArmyExperience>();
        assert_eq!((xp.get("GER"), xp.get("FRA")), (14, 14), "резерв не воюет и опыт не приносит");
    }

    #[test]
    fn divisions_on_the_line_earn_personal_experience_each_hour() {
        // 6 атакующих и 5 защитников: на линии по 4, остальные в резерве
        let (mut app, att, def) = scenario(&[1; 6], 5, 1);
        for _ in 0..24 {
            tick(&mut app);
        }
        let mut on_line = 0;
        let mut in_reserve = 0;
        for e in att.iter().chain(def.iter()) {
            let Some(d) = app.world().get::<Division>(*e) else { continue };
            match d.combat_state {
                CombatState::Active => {
                    assert_eq!(d.experience_pct, 4, "24 часа на линии = +4%");
                    on_line += 1;
                }
                CombatState::Reserve => {
                    assert_eq!(d.experience_pct, 0, "резерв не набирает опыт");
                    in_reserve += 1;
                }
                CombatState::None => {}
            }
        }
        assert_eq!((on_line, in_reserve), (8, 3));
    }

    /// Потери организации (в тысячных) у защитников / атакующих после одного часа боя 1 на 1 с заданными тактиками.
    fn org_losses(att_tactic: CombatTactic, def_tactic: CombatTactic) -> (u32, u32) {
        let (mut app, att, def) = scenario(&[1], 1, 1);
        {
            let world = app.world_mut();
            let mut combat = world.query::<&mut ActiveCombat>().single_mut(world);
            combat.attacker_tactic = att_tactic;
            combat.defender_tactic = def_tactic;
        }
        tick(&mut app);
        let lost = |app: &App, e: Entity| {
            let d = app.world().get::<Division>(e).expect("дивизия");
            d.max_org * 1000 - d.org_milli()
        };
        (lost(&app, def[0]), lost(&app, att[0]))
    }

    #[test]
    fn tactics_change_damage_inside_resolve_combats() {
        use CombatTactic::*;
        let (base_def_loss, base_att_loss) = org_losses(Attack, Defend);
        // штурм (+25%) бьёт сильнее базовой атаки (+5%)
        let (assault_def_loss, _) = org_losses(Assault, Defend);
        assert!(assault_def_loss > base_def_loss, "{assault_def_loss} > {base_def_loss}");
        // контратака защитника (+25% к его атаке) сильнее бьёт по атакующим
        let (_, counter_att_loss) = org_losses(Attack, CounterAttack);
        assert!(counter_att_loss > base_att_loss);
        // отход парирует штурм: атакующий теряет бонус и бьёт на 25% слабее, защитник бьёт на 6% сильнее
        let (withdraw_def_loss, withdraw_att_loss) = org_losses(Assault, TacticalWithdrawal);
        assert!(withdraw_def_loss < base_def_loss, "парированный штурм слабее даже базовой атаки");
        assert!(withdraw_att_loss > base_att_loss, "за парирование защитник получает +25% (с учётом отхода 106%)");
        // охват против контратаки: парирована контратака — защитник теряет свой бонус, охват бьёт на 25% сильнее
        let (enc_def_loss, enc_att_loss) = org_losses(Encirclement, CounterAttack);
        let (assault_vs_counter_def, assault_vs_counter_att) = org_losses(Assault, CounterAttack);
        assert!(enc_def_loss > assault_vs_counter_def, "парирование охватом добавляет +25% урона");
        assert!(enc_att_loss < assault_vs_counter_att, "парированная контратака потеряла +25% к атаке");
    }

    fn make_neutral(id: u32) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(id, "d".into(), "GER".into(), 1, PLAIN_INFANTRY_TEMPLATE.to_string(), templates.get_or_base(PLAIN_INFANTRY_TEMPLATE))
            .trained()
    }

    #[test]
    fn countered_side_loses_tactic_bonuses_in_the_formula() {
        use CombatTactic::*;
        let (attacker, defender) = (make_neutral(1), make_neutral(2));
        let formula = |att: CombatTactic, def: CombatTactic| {
            let a = side_mods(att, def, true);
            let d = side_mods(att, def, false);
            combat_bp_ctx(&attacker, &defender, 0, HitContext::NONE.with_tactics(a.attack_pct, d.defense_pct))
        };
        // без тактик: 800 * 90 / 315 = 228
        assert_eq!(combat_bp(&attacker, &defender, 0), 228);
        // штурм +25%: атака 112, защита 225 (контратака защиты не даёт) -> 800 * 112 / 337 = 265
        assert_eq!(formula(Assault, CounterAttack), 265);
        // оборона +5%: защита 236 -> 800 * 112 / 348 = 257
        assert_eq!(formula(Assault, Defend), 257);
        // отход парирует штурм: бонус атаки пропал -> как без тактик
        assert_eq!(formula(Assault, TacticalWithdrawal), 228);
        // базовая атака +5% (94) против обороны +5% (236)
        assert_eq!(formula(Attack, Defend), 800 * 94 / (94 + 236));
    }

    #[test]
    fn tactic_choice_is_deterministic_and_respects_side_pools() {
        // одни и те же входы -> один и тот же выбор
        let first = choose_tactics(42, 13_000, 7, 101, None, false, false);
        assert_eq!(first, choose_tactics(42, 13_000, 7, 101, None, false, false));
        // тактики сторон из своих наборов
        let mut seen_attackers = BTreeSet::new();
        let mut seen_defenders = BTreeSet::new();
        for day in 0..60 {
            let (a, d) = choose_tactics(42, day, 7, 101, None, false, false);
            assert!(ATTACKER_TACTICS.contains(&a) && DEFENDER_TACTICS.contains(&d));
            seen_attackers.insert(a);
            seen_defenders.insert(d);
            // при разведке у атакующих порядок выбора другой, но наборы те же
            let (a2, d2) = choose_tactics(42, day, 7, 101, Some(true), false, false);
            assert!(ATTACKER_TACTICS.contains(&a2) && DEFENDER_TACTICS.contains(&d2));
        }
        assert_eq!((seen_attackers.len(), seen_defenders.len()), (3, 3), "за два месяца встречаются все тактики");
        // другой бой или другой командующий дают другую последовательность
        let sequence = |bits: u64, key: u32| (0..40).map(|day| choose_tactics(bits, day, key, 101, None, false, false)).collect::<Vec<_>>();
        assert_ne!(sequence(42, 7), sequence(43, 7));
        assert_ne!(sequence(42, 7), sequence(42, 8));
    }

    #[test]
    fn second_picker_prefers_the_counter_pick() {
        use CombatTactic::*;
        // атакующие выбрали штурм: отход защитника весит 2 из 4
        let mut counts = BTreeMap::new();
        for roll in 0..4u64 {
            *counts.entry(crate::command::pick_tactic(&DEFENDER_TACTICS, Some(Assault), roll)).or_insert(0) += 1;
        }
        assert_eq!(counts[&TacticalWithdrawal], 2);
        assert_eq!((counts[&Defend], counts[&CounterAttack]), (1, 1));
        // против обычной атаки контр-пика нет: веса равны
        let flat: Vec<CombatTactic> = (0..3u64).map(|r| crate::command::pick_tactic(&DEFENDER_TACTICS, Some(Attack), r)).collect();
        assert_eq!(flat, DEFENDER_TACTICS.to_vec());
        // контратаку защитника парирует охват
        let encircle = (0..4u64).filter(|&r| crate::command::pick_tactic(&ATTACKER_TACTICS, Some(CounterAttack), r) == Encirclement).count();
        assert_eq!(encircle, 2);
    }

    #[test]
    fn daily_tick_selects_tactics_and_marks_counters_deterministically() {
        let run = || {
            let (mut app, _att, _def) = scenario(&[1, 1], 2, 1);
            app.add_event::<DailyTickEvent>().add_systems(Update, select_combat_tactics);
            let mut seen = Vec::new();
            for day in 1..=20u32 {
                app.world_mut().send_event(DailyTickEvent { day, month: 3, year: 1936 });
                app.update();
                let world = app.world_mut();
                let combat = world.query::<&ActiveCombat>().single(world);
                let (att_c, def_c) = resolve_counters(combat.attacker_tactic, combat.defender_tactic);
                assert_eq!((combat.is_attacker_countered, combat.is_defender_countered), (att_c, def_c));
                assert!(!(att_c && def_c), "парирована может быть только одна сторона");
                assert_eq!(combat.tactic_change_day, day_index(day, 3, 1936));
                seen.push((combat.attacker_tactic, combat.defender_tactic));
            }
            seen
        };
        let first = run();
        assert_eq!(first, run(), "тот же бой и те же сутки дают тот же выбор");
        assert!(first.iter().collect::<BTreeSet<_>>().len() > 1, "тактики меняются от суток к суткам");
    }

    #[test]
    fn tactic_ui_marks_the_countered_side_in_red() {
        use CombatTactic::*;
        let (mut app, _att, _def) = scenario(&[1], 1, 1);
        let world = app.world_mut();
        let mut combat = world.query::<&mut ActiveCombat>().single_mut(world);
        combat.attacker_tactic = Assault;
        combat.defender_tactic = TacticalWithdrawal;
        let segments = tactics_segments(&combat);
        let text: String = segments.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "Тактика: Штурм [ПАРИРОВАНО!] vs Отход");
        assert_eq!(segments.iter().filter(|(_, red)| *red).count(), 1);
        combat.attacker_tactic = Encirclement;
        combat.defender_tactic = CounterAttack;
        let text: String = tactics_segments(&combat).iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "Тактика: Охват vs Контратака [ПАРИРОВАНО!]");
        combat.attacker_tactic = Attack;
        combat.defender_tactic = Defend;
        let text: String = tactics_segments(&combat).iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "Тактика: Атака vs Оборона");
    }

    fn set_phase(app: &mut App, phase: CombatPhase, hours: u32) {
        let world = app.world_mut();
        let mut combat = world.query::<&mut ActiveCombat>().single_mut(world);
        combat.phase = phase;
        combat.phase_hours_left = hours;
    }

    fn combat_of(app: &mut App) -> ActiveCombat {
        let world = app.world_mut();
        world.query::<&ActiveCombat>().single(world).clone()
    }

    #[test]
    fn phase_machine_follows_the_defenders_choice() {
        use CombatPhase as P;
        use CombatTactic as T;
        // отход защитника включает фазу на 24 часа (и продлевает её, если выбран снова)
        assert_eq!(next_phase(P::Default, 0, T::TacticalWithdrawal), (P::TacticalWithdrawal, 24));
        assert_eq!(next_phase(P::TacticalWithdrawal, 3, T::TacticalWithdrawal), (P::TacticalWithdrawal, 24));
        // срок истёк, отход не выбран — обычная фаза
        assert_eq!(next_phase(P::TacticalWithdrawal, 0, T::Defend), (P::Default, 0));
        assert_eq!(next_phase(P::TacticalWithdrawal, 0, T::CounterAttack), (P::Default, 0));
        // срок ещё идёт — фаза доживает своё время, даже если тактика уже сменилась
        assert_eq!(next_phase(P::TacticalWithdrawal, 7, T::Defend), (P::TacticalWithdrawal, 7));
        // обычная фаза остаётся обычной
        assert_eq!(next_phase(P::Default, 0, T::Defend), (P::Default, 0));
    }

    #[test]
    fn daily_choice_of_withdrawal_starts_a_24_hour_phase() {
        let (mut app, _att, _def) = scenario(&[1], 1, 1);
        app.add_event::<DailyTickEvent>().add_systems(Update, select_combat_tactics);
        let mut started = false;
        for day in 1..=60u32 {
            app.world_mut().send_event(DailyTickEvent { day, month: 4, year: 1936 });
            app.update();
            let combat = combat_of(&mut app);
            if combat.defender_tactic == CombatTactic::TacticalWithdrawal {
                assert_eq!((combat.phase, combat.phase_hours_left), (CombatPhase::TacticalWithdrawal, 24));
                started = true;
                break;
            }
            assert_eq!(combat.phase, CombatPhase::Default, "без отхода фаза обычная");
        }
        assert!(started, "за два месяца защитники хотя бы раз выбрали отход");
    }

    #[test]
    fn withdrawal_phase_halves_the_front_and_pushes_extra_attackers_to_reserve() {
        let (mut app, att, _def) = scenario(&[1; 4], 2, 1);
        tick(&mut app);
        let states = |app: &App| -> Vec<CombatState> {
            att.iter().map(|&e| app.world().get::<Division>(e).expect("дивизия").combat_state).collect()
        };
        assert_eq!(states(&app), vec![CombatState::Active; 4], "4 * 18 = 72 влезают в 80");
        assert_eq!(combat_of(&mut app).effective_max_width(), 80);
        set_phase(&mut app, CombatPhase::TacticalWithdrawal, 24);
        assert_eq!(combat_of(&mut app).effective_max_width(), 40, "лимит вдвое меньше");
        tick(&mut app);
        use CombatState::{Active, Reserve};
        assert_eq!(states(&app), vec![Active, Active, Reserve, Reserve], "лишние с конца по id ушли в резерв");
        // резерв не возвращается на линию, пока фронт узкий
        for _ in 0..10 {
            tick(&mut app);
        }
        assert_eq!(states(&app)[2..], [Reserve, Reserve]);
        // с обычной фазой ширина возвращается, и резерв может снова выйти на линию
        set_phase(&mut app, CombatPhase::Default, 0);
        assert_eq!(combat_of(&mut app).effective_max_width(), 80);
    }

    #[test]
    fn phase_timer_counts_down_hourly_and_reaches_zero_after_24_hours() {
        let (mut app, _att, _def) = scenario(&[1; 4], 4, 1);
        set_phase(&mut app, CombatPhase::TacticalWithdrawal, WITHDRAWAL_PHASE_HOURS);
        tick(&mut app);
        assert_eq!(combat_of(&mut app).phase_hours_left, 23);
        for _ in 0..23 {
            tick(&mut app);
        }
        let combat = combat_of(&mut app);
        assert_eq!(combat.phase_hours_left, 0, "ровно 24 часа");
        // сама фаза снимается суточным выбором (next_phase), счётчик не уходит ниже нуля
        tick(&mut app);
        assert_eq!(combat_of(&mut app).phase_hours_left, 0);
        let (phase, left) = next_phase(combat.phase, combat.phase_hours_left, CombatTactic::Defend);
        assert_eq!((phase, left), (CombatPhase::Default, 0));
    }

    #[test]
    fn rear_guard_phase_cuts_damage_to_defenders_organisation() {
        use CombatTactic::*;
        // атака против обороны: одинаковые тактики, различается только фаза
        let loss = |phase: CombatPhase| {
            let (mut app, _att, def) = scenario(&[1], 1, 1);
            set_phase(&mut app, phase, 24);
            tick(&mut app);
            let d = app.world().get::<Division>(def[0]).expect("защитник");
            d.max_org * 1000 - d.org_milli()
        };
        let (normal, withdrawal) = (loss(CombatPhase::Default), loss(CombatPhase::TacticalWithdrawal));
        assert!(withdrawal < normal, "{withdrawal} < {normal}");
        // ровно три четверти (округление вниз по сотой доле)
        assert!(withdrawal.abs_diff(normal * 75 / 100) <= 2, "{withdrawal} против {}", normal * 75 / 100);
        let _ = (Attack, Defend);
    }

    #[test]
    fn phase_status_text_for_the_ui() {
        let (mut app, _att, _def) = scenario(&[1], 1, 1);
        assert_eq!(phase_text(&combat_of(&mut app)), None, "в обычной фазе строки нет");
        set_phase(&mut app, CombatPhase::TacticalWithdrawal, 17);
        assert_eq!(
            phase_text(&combat_of(&mut app)).as_deref(),
            Some("Фаза: Тактический отход (осталось 17 ч.) | Фронт сужен на 50%")
        );
    }

    /// Бой через реку создаётся настоящей системой `advance_marches`: атакующая дивизия приходит из провинции 1.
    fn river_battle(river: bool) -> ActiveCombat {
        let mut app = App::new();
        let neighbors = BTreeMap::from([(1, BTreeSet::from([2])), (2, BTreeSet::from([1]))]);
        app.add_event::<HourTick>()
            .add_event::<ProvinceControllerChanged>()
            .insert_resource(ProvinceMap::empty_for_test())
            .insert_resource(ProvinceAdjacency { neighbors })
            .insert_resource(ProvinceControllers(BTreeMap::from([(1, "GER".to_string()), (2, "FRA".to_string())])))
            .insert_resource(ProvinceRivers(if river { BTreeSet::from([(1, 2)]) } else { BTreeSet::new() }))
            .add_systems(Update, advance_marches);
        let m = MovementProgress { from: 1, to: 2, hours_left: 1, total_hours: 24 };
        spawn_division(&mut app, 1, "GER", 1, Some(m), 1);
        spawn_division(&mut app, 101, "FRA", 2, None, 1);
        tick(&mut app);
        combat_of(&mut app)
    }

    #[test]
    fn river_crossing_starts_the_battle_in_the_bridge_phase() {
        let combat = river_battle(true);
        assert_eq!(combat.phase, CombatPhase::Bridge);
        assert_eq!(combat.phase_hours_left, 48);
        assert_eq!((combat.attacker_tactic, combat.defender_tactic), (CombatTactic::SeizeBridge, CombatTactic::HoldBridge));
        assert!(combat.is_attacker_countered && !combat.is_defender_countered, "удержание парирует захват");
        assert!(!combat.bridge_held);
        // без реки — обычный бой
        let plain = river_battle(false);
        assert_eq!(plain.phase, CombatPhase::Default);
        assert_eq!((plain.attacker_tactic, plain.defender_tactic), (CombatTactic::Attack, CombatTactic::Defend));
    }

    #[test]
    fn seize_bridge_removes_the_crossing_penalty() {
        let (attacker, defender) = (make_neutral(1), make_neutral(2));
        let seize = side_mods(CombatTactic::SeizeBridge, CombatTactic::RetakeBridge, true);
        let defend = side_mods(CombatTactic::SeizeBridge, CombatTactic::RetakeBridge, false);
        assert_eq!((seize.attack_pct, seize.damage_pct), (20, 100), "захват не парирован");
        assert_eq!(defend.attack_pct, 15, "контрудар защитника +15%");
        let strike = |river_penalty: u32| {
            let ctx = HitContext { attack_penalty: river_penalty, ..HitContext::NONE }.with_tactics(seize.attack_pct, 0);
            combat_bp_ctx(&attacker, &defender, 0, ctx)
        };
        // мост взят: штраф 0, атака 90 * 120 / 100 = 108 -> 800 * 108 / 333 = 259
        assert_eq!(strike(0), 259);
        // мост у обороны: штраф -30% (атака 63 -> 75) -> 800 * 75 / 300 = 200
        assert_eq!(strike(30), 200);
        // в бою: пока мост не взят, наступающий бьёт слабее, чем когда `bridge_held`
        let loss = |held: bool| {
            let (mut app, _att, def) = scenario(&[1], 1, 1);
            app.world_mut().resource_mut::<ProvinceRivers>().0.insert((1, 2));
            {
                let world = app.world_mut();
                let mut combat = world.query::<&mut ActiveCombat>().single_mut(world);
                combat.phase = CombatPhase::Bridge;
                combat.phase_hours_left = 48;
                combat.attacker_tactic = CombatTactic::SeizeBridge;
                combat.defender_tactic = CombatTactic::RetakeBridge;
                combat.bridge_held = held;
            }
            tick(&mut app);
            let d = app.world().get::<Division>(def[0]).expect("защитник");
            d.max_org * 1000 - d.org_milli()
        };
        assert!(loss(true) > loss(false), "{} > {}", loss(true), loss(false));
    }

    #[test]
    fn hold_bridge_keeps_the_penalty_and_adds_twenty_percent_defense() {
        use CombatTactic::*;
        let hold = side_mods(SeizeBridge, HoldBridge, false);
        assert_eq!((hold.defense_pct, hold.damage_pct), (20, 125), "защита +20%, парирование даёт ещё +25% урона");
        let seize = side_mods(SeizeBridge, HoldBridge, true);
        assert_eq!(seize.attack_pct, 0, "парированный захват теряет бонус");
        // мост остаётся у обороны, штраф реки на месте
        assert!(!bridge_held_after(false, SeizeBridge, HoldBridge));
        assert!(!bridge_held_after(true, SeizeBridge, HoldBridge), "удержание перехватывает мост");
        // формула: атака 63 (штраф -30%), защита 225 * 120 / 100 = 270 -> 800 * 63 / 333 = 151
        let (attacker, defender) = (make_neutral(1), make_neutral(2));
        let ctx = HitContext { attack_penalty: 30, ..HitContext::NONE }.with_tactics(seize.attack_pct, hold.defense_pct);
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, ctx), 151);
        // без удержания защита ниже: 800 * 63 / 288 = 175
        let ctx = HitContext { attack_penalty: 30, ..HitContext::NONE };
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, ctx), 175);
    }

    #[test]
    fn bridge_ownership_follows_the_tactics() {
        use CombatTactic::*;
        assert!(bridge_held_after(false, SeizeBridge, RetakeBridge), "захват без парирования берёт мост");
        assert!(!bridge_held_after(true, SeizeBridge, RetakeBridge), "контрудар выбивает с уже взятого моста");
        assert_eq!(SeizeBridge.counter_tactic(), Some(HoldBridge));
        assert_eq!(HoldBridge.counter_tactic(), Some(RetakeBridge));
        assert_eq!(RetakeBridge.counter_tactic(), None);
        assert_eq!(
            [SeizeBridge, HoldBridge, RetakeBridge].iter().map(CombatTactic::title).collect::<Vec<_>>(),
            vec!["Захват моста", "Удержание моста", "Контрудар за мост"]
        );
    }

    #[test]
    fn engineers_double_the_weight_of_seize_bridge() {
        use CombatTactic::*;
        assert_eq!(crate::command::tactic_weight(SeizeBridge, None, None), 1);
        assert_eq!(crate::command::tactic_weight(SeizeBridge, None, Some(SeizeBridge)), 2, "сапёры: вес ×2");
        assert_eq!(crate::command::tactic_weight(Attack, None, Some(SeizeBridge)), 1, "другие тактики не затронуты");
        // выбор из [захват, атака]: без сапёров 1 из 2, с сапёрами 2 из 3
        let pool = [SeizeBridge, Attack];
        let seized = |boost: Option<CombatTactic>, rolls: u64| {
            (0..rolls).filter(|&r| pick_tactic_boosted(&pool, None, r, boost) == SeizeBridge).count()
        };
        assert_eq!(seized(None, 2), 1);
        assert_eq!(seized(Some(SeizeBridge), 3), 2);
    }

    #[test]
    fn bridge_phase_selects_only_river_tactics_deterministically() {
        for engineers in [false, true] {
            let mut defenders = BTreeSet::new();
            for day in 0..60 {
                let (a, d) = choose_bridge_tactics(9, day, 1, 101, None, engineers);
                assert_eq!(a, CombatTactic::SeizeBridge);
                assert!(BRIDGE_DEFENDER_TACTICS.contains(&d));
                assert_eq!((a, d), choose_bridge_tactics(9, day, 1, 101, None, engineers));
                defenders.insert(d);
            }
            assert_eq!(defenders.len(), 2, "защитники выбирают и удержание, и контрудар");
        }
    }

    #[test]
    fn daily_choice_in_bridge_phase_sets_the_bridge_and_leaves_the_phase() {
        let (mut app, _att, _def) = scenario(&[1], 1, 1);
        app.add_event::<DailyTickEvent>().add_systems(Update, select_combat_tactics);
        let mut released = 0;
        let mut kept = 0;
        for day in 1..=40u32 {
            // срок речной фазы вышел: выход зависит от того, взят ли мост
            set_phase(&mut app, CombatPhase::Bridge, 0);
            app.world_mut().send_event(DailyTickEvent { day, month: 5, year: 1936 });
            app.update();
            let combat = combat_of(&mut app);
            assert_eq!(combat.attacker_tactic, CombatTactic::SeizeBridge);
            assert!(BRIDGE_DEFENDER_TACTICS.contains(&combat.defender_tactic));
            if combat.bridge_held {
                assert_eq!((combat.phase, combat.phase_hours_left), (CombatPhase::Default, 0));
                released += 1;
            } else {
                assert_eq!((combat.phase, combat.phase_hours_left), (CombatPhase::Bridge, 48), "мост не взят: фаза продлена");
                kept += 1;
            }
            // сбрасываем мост на следующий проход
            let world = app.world_mut();
            world.query::<&mut ActiveCombat>().single_mut(world).bridge_held = false;
        }
        assert!(released > 0 && kept > 0, "встречаются оба исхода: {released} / {kept}");
        // пока срок идёт, фаза не меняется
        assert_eq!(next_bridge_phase(true, 20), (CombatPhase::Bridge, 20));
        assert_eq!(next_bridge_phase(false, 0), (CombatPhase::Bridge, 48));
        assert_eq!(next_bridge_phase(true, 0), (CombatPhase::Default, 0));
    }

    #[test]
    fn bridge_phase_timer_and_status_text() {
        let (mut app, _att, _def) = scenario(&[1; 4], 4, 1);
        set_phase(&mut app, CombatPhase::Bridge, BRIDGE_PHASE_HOURS);
        for _ in 0..24 {
            tick(&mut app);
        }
        assert_eq!(combat_of(&mut app).phase_hours_left, 24, "таймер идёт по часовым тикам");
        let mut combat = combat_of(&mut app);
        assert_eq!(phase_text(&combat).as_deref(), Some("Фаза: Борьба за мост | Мост удерживается обороной"));
        combat.bridge_held = true;
        assert_eq!(phase_text(&combat).as_deref(), Some("Фаза: Борьба за мост | Мост захвачен (штраф реки снят)"));
    }

    fn named_division(name: &str) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(1, "d".into(), "GER".into(), 1, name.to_string(), templates.get_or_base(name)).trained()
    }

    #[test]
    fn urban_terrain_hits_attackers_harder_and_armor_hardest() {
        let urban = terrain_modifiers(Some("urban"));
        assert_eq!((urban.defense_bonus, urban.attack_penalty, urban.armored_attack_penalty), (30, 30, 20));
        let infantry = named_division(PLAIN_INFANTRY_TEMPLATE);
        let mut tanks = named_division("Panzer-Division");
        tanks.hardness = 60;
        assert_eq!(HitContext::for_attack(&urban, &infantry, false).attack_penalty, 30, "пехота: -30%");
        assert_eq!(HitContext::for_attack(&urban, &tanks, false).attack_penalty, 50, "бронетехника: -30% и ещё -20%");
        // защита обороняющихся +30%: атака 90 * 70 / 100 = 63, защита 225 * 130 / 100 = 292 -> 800 * 63 / 355 = 141
        let (attacker, defender) = (make_neutral(1), make_neutral(2));
        let ctx = HitContext::for_attack(&urban, &infantry, false);
        assert_eq!(ctx.defense_bonus, 30);
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, ctx), 141);
    }

    #[test]
    fn armor_gives_no_advantage_at_point_blank() {
        let infantry = named_division(BASE_TEMPLATE);
        let panzer = named_division("Panzer-Division");
        // обычный бой: пехота по танкам вдвое слабее, танки по пехоте на 50% сильнее
        assert_eq!(apply_armor_rule(1000, &infantry, &panzer, false), 500);
        assert_eq!(apply_armor_rule(1000, &panzer, &infantry, false), 1500);
        // в упор бонус брони аннулирован для обеих сторон
        assert_eq!(apply_armor_rule(1000, &infantry, &panzer, true), 1000);
        assert_eq!(apply_armor_rule(1000, &panzer, &infantry, true), 1000);
    }

    #[test]
    fn close_quarters_tactics_and_texts() {
        use CombatTactic::*;
        assert_eq!(CloseQuartersAttack.counter_tactic(), Some(CQTacticalWithdrawal));
        assert_eq!(CQTacticalWithdrawal.counter_tactic(), None);
        assert_eq!(CloseQuartersAttack.title(), "Ближний бой");
        assert_eq!(CQTacticalWithdrawal.title(), "Разрыв дистанции");
        // атака в упор: +10% атаки, оборона бьёт на 5% сильнее
        assert_eq!(side_mods(CloseQuartersAttack, Defend, true), crate::command::TacticMods { attack_pct: 10, defense_pct: 0, damage_pct: 100 });
        assert_eq!(side_mods(CloseQuartersAttack, Defend, false), crate::command::TacticMods { attack_pct: 0, defense_pct: 5, damage_pct: 105 });
        // разрыв дистанции парирует атаку: бонусов нет, парирующий бьёт на 25% сильнее (с учётом отхода: 106%)
        assert_eq!(side_mods(CloseQuartersAttack, CQTacticalWithdrawal, true), crate::command::TacticMods { attack_pct: 0, defense_pct: 0, damage_pct: 75 });
        assert_eq!(side_mods(CloseQuartersAttack, CQTacticalWithdrawal, false), crate::command::TacticMods { attack_pct: 0, defense_pct: 0, damage_pct: 106 });
        let (mut app, _a, _d) = scenario(&[1], 1, 1);
        set_phase(&mut app, CombatPhase::CloseQuarters, 17);
        assert_eq!(
            phase_text(&combat_of(&mut app)).as_deref(),
            Some("Фаза: Ближний бой (осталось 17 ч.) | Фронт сужен на 50% | Урон повышен")
        );
    }

    #[test]
    fn urban_assault_weight_triples_and_phase_machine_follows_it() {
        use CombatTactic::*;
        assert_eq!(tactic_weight_scaled_for_test(Assault, true), 3);
        assert_eq!(tactic_weight_scaled_for_test(Assault, false), 1);
        // пул [атака, штурм, охват]: в городе штурм берёт 3 из 5 бросков
        let pool = [Attack, Assault, Encirclement];
        let assaults = (0..5u64)
            .filter(|&r| pick_tactic_scaled(&pool, None, r, Some((Assault, URBAN_ASSAULT_WEIGHT_MULT))) == Assault)
            .count();
        assert_eq!(assaults, 3);
        // переходы: штурм в городе включает/продлевает 24 часа; срок вышел без штурма — обычная фаза
        use CombatPhase::*;
        assert_eq!(next_cq_phase(Default, 0, Assault, true), (CloseQuarters, 24));
        assert_eq!(next_cq_phase(CloseQuarters, 5, Assault, true), (CloseQuarters, 24), "штурм продлён");
        assert_eq!(next_cq_phase(Default, 0, Assault, false), (Default, 0), "вне города штурм фазу не включает");
        assert_eq!(next_cq_phase(CloseQuarters, 5, CloseQuartersAttack, true), (CloseQuarters, 5), "срок ещё идёт");
        assert_eq!(next_cq_phase(CloseQuarters, 0, CloseQuartersAttack, true), (Default, 0), "срок вышел");
        assert_eq!(next_cq_phase(CombatPhase::TacticalWithdrawal, 9, Assault, true), (CloseQuarters, 24), "штурм города приоритетнее отхода");
    }

    fn tactic_weight_scaled_for_test(tactic: CombatTactic, urban: bool) -> u64 {
        let scale = urban.then_some((CombatTactic::Assault, URBAN_ASSAULT_WEIGHT_MULT));
        crate::command::tactic_weight_scaled(tactic, None, scale)
    }

    #[test]
    fn assault_in_an_urban_province_starts_close_quarters() {
        let (mut app, _att, _def) = scenario(&[1], 1, 1);
        app.insert_resource(ProvinceMap::with_terrain_for_test(&[(2, "urban")]));
        app.add_event::<DailyTickEvent>().add_systems(Update, select_combat_tactics);
        let mut started = false;
        for day in 1..=60u32 {
            set_phase(&mut app, CombatPhase::Default, 0);
            app.world_mut().send_event(DailyTickEvent { day, month: 6, year: 1936 });
            app.update();
            let combat = combat_of(&mut app);
            if combat.attacker_tactic == CombatTactic::Assault {
                assert_eq!((combat.phase, combat.phase_hours_left), (CombatPhase::CloseQuarters, 24));
                started = true;
                break;
            }
            assert_ne!(combat.phase, CombatPhase::CloseQuarters, "без штурма уличного боя нет");
        }
        assert!(started, "в городе штурм встречается");
    }

    #[test]
    fn open_ground_assault_never_starts_close_quarters() {
        let (mut app, _att, _def) = scenario(&[1], 1, 1);
        app.add_event::<DailyTickEvent>().add_systems(Update, select_combat_tactics);
        for day in 1..=60u32 {
            set_phase(&mut app, CombatPhase::Default, 0);
            app.world_mut().send_event(DailyTickEvent { day, month: 6, year: 1936 });
            app.update();
            assert_ne!(combat_of(&mut app).phase, CombatPhase::CloseQuarters, "равнина: фазы ближнего боя нет");
        }
    }

    #[test]
    fn close_quarters_phase_selects_its_own_tactics() {
        for day in 0..40 {
            let (a, d) = choose_cq_tactics(5, day, 1, 101, None);
            assert!(CQ_ATTACKER_TACTICS.contains(&a) && CQ_DEFENDER_TACTICS.contains(&d));
            assert_eq!((a, d), choose_cq_tactics(5, day, 1, 101, None));
        }
    }

    #[test]
    fn close_quarters_halves_the_front_and_pushes_extra_divisions_to_reserve() {
        let (mut app, att, _def) = scenario(&[1; 4], 2, 1);
        tick(&mut app);
        let states = |app: &App| -> Vec<CombatState> {
            att.iter().map(|&e| app.world().get::<Division>(e).expect("дивизия").combat_state).collect()
        };
        assert_eq!(states(&app), vec![CombatState::Active; 4]);
        set_phase(&mut app, CombatPhase::CloseQuarters, 24);
        assert_eq!(combat_of(&mut app).effective_max_width(), 40);
        tick(&mut app);
        use CombatState::{Active, Reserve};
        assert_eq!(states(&app), vec![Active, Active, Reserve, Reserve]);
    }

    #[test]
    fn close_quarters_raises_damage_on_both_sides_and_counts_down() {
        let losses = |phase: CombatPhase| {
            let (mut app, att, def) = scenario(&[1], 1, 1);
            set_phase(&mut app, phase, 24);
            tick(&mut app);
            let lost = |e: Entity| {
                let d = app.world().get::<Division>(e).expect("дивизия");
                d.max_org * 1000 - d.org_milli()
            };
            (lost(def[0]), lost(att[0]), combat_of(&mut app).phase_hours_left)
        };
        let (normal_def, normal_att, _) = losses(CombatPhase::Default);
        let (cq_def, cq_att, hours) = losses(CombatPhase::CloseQuarters);
        assert!(cq_def.abs_diff(normal_def * 115 / 100) <= 2, "{cq_def} против {}", normal_def * 115 / 100);
        assert!(cq_att.abs_diff(normal_att * 115 / 100) <= 2, "{cq_att} против {}", normal_att * 115 / 100);
        assert_eq!(hours, 23, "часовой декремент");
    }

    #[test]
    fn breakthrough_requires_armor_or_a_better_commander() {
        use crate::command::breakthrough_available as available;
        // танковый клин: твёрдость выше 50% — доступен даже без командиров
        assert!(available(60, 0, 0, 1, 101), "бронетехника");
        assert!(available(51, 0, 0, 1, 101));
        // граница: ровно 50% — ещё не клин
        assert!(!available(50, 0, 0, 1, 101));
        // чистая пехота (0%) и равные командиры при меньшем id генерала — недоступен
        assert!(!available(0, 0, 0, 1, 101), "пехота без преимущества командира");
        assert!(!available(0, 3, 5, 1, 101), "командир слабее");
        // командир сильнее — доступен и пехоте
        assert!(available(0, 6, 5, 1, 101));
        // при равных навыках решает больший id ведущего генерала
        assert!(available(0, 4, 4, 200, 101));
        assert!(!available(0, 4, 4, 100, 101));
    }

    #[test]
    fn pure_infantry_never_picks_breakthrough_but_armor_does() {
        use CombatTactic::*;
        // пул без прорыва: ни прорыва, ни засады
        for day in 0..200 {
            let (a, d) = choose_tactics(77, day, 1, 101, None, false, false);
            assert!(a != Breakthrough && d != Ambush);
        }
        // пул с прорывом: оба появляются, остальные тактики остаются
        let (mut seen_breakthrough, mut seen_ambush, mut seen_assault) = (false, false, false);
        for day in 0..300 {
            let (a, d) = choose_tactics(77, day, 1, 101, None, false, true);
            seen_breakthrough |= a == Breakthrough;
            seen_ambush |= d == Ambush;
            seen_assault |= a == Assault;
            assert!(ATTACKER_TACTICS_WITH_BREAKTHROUGH.contains(&a) && DEFENDER_TACTICS_WITH_AMBUSH.contains(&d));
        }
        assert!(seen_breakthrough && seen_ambush && seen_assault);
    }

    #[test]
    fn daily_selection_blocks_breakthrough_for_infantry_and_allows_it_for_armor() {
        use CombatTactic::*;
        // пехота (твёрдость 0, командиров нет, id атакующего меньше): прорыв никогда не выбирается
        let (mut app, _att, _def) = scenario(&[1; 2], 2, 1);
        app.add_event::<DailyTickEvent>().add_systems(Update, select_combat_tactics);
        for day in 1..=120u32 {
            app.world_mut().send_event(DailyTickEvent { day, month: 7, year: 1936 });
            app.update();
            let combat = combat_of(&mut app);
            assert!(combat.attacker_tactic != Breakthrough && combat.defender_tactic != Ambush, "день {day}");
        }
        // танковый клин: твёрдость 60% у атакующих
        let (mut app, att, _def) = scenario(&[1; 2], 2, 1);
        for &e in &att {
            app.world_mut().get_mut::<Division>(e).expect("дивизия").hardness = 60;
        }
        app.add_event::<DailyTickEvent>().add_systems(Update, select_combat_tactics);
        let (mut free, mut parried) = (false, false);
        for day in 1..=300u32 {
            set_phase(&mut app, CombatPhase::Default, 0);
            app.world_mut().send_event(DailyTickEvent { day, month: 7, year: 1936 });
            app.update();
            let combat = combat_of(&mut app);
            if combat.attacker_tactic == Breakthrough {
                if combat.is_attacker_countered {
                    assert_ne!(combat.phase, CombatPhase::Breakthrough, "парированный прорыв фазу не включает");
                    parried = true;
                } else {
                    assert_eq!((combat.phase, combat.phase_hours_left), (CombatPhase::Breakthrough, 24));
                    free = true;
                }
            }
        }
        assert!(free && parried, "встречаются оба исхода: {free} / {parried}");
    }

    #[test]
    fn ambush_parries_breakthrough() {
        use CombatTactic::*;
        assert_eq!(Breakthrough.counter_tactic(), Some(Ambush));
        assert_eq!(Ambush.counter_tactic(), None);
        assert_eq!(resolve_counters(Breakthrough, Ambush), (true, false));
        assert_eq!(resolve_counters(Breakthrough, Defend), (false, false));
        assert_eq!((Breakthrough.title(), Ambush.title()), ("Прорыв", "Засада"));
        // прорыв без парирования: +25% атаки, защитник бьёт на 15% слабее
        assert_eq!(side_mods(Breakthrough, Defend, true), crate::command::TacticMods { attack_pct: 25, defense_pct: 0, damage_pct: 100 });
        assert_eq!(side_mods(Breakthrough, Defend, false), crate::command::TacticMods { attack_pct: 0, defense_pct: 5, damage_pct: 85 });
        // засада: бонус прорыва пропал, защита +25%, парирование даёт +25% урона
        assert_eq!(side_mods(Breakthrough, Ambush, true), crate::command::TacticMods { attack_pct: 0, defense_pct: 0, damage_pct: 100 });
        assert_eq!(side_mods(Breakthrough, Ambush, false), crate::command::TacticMods { attack_pct: 0, defense_pct: 25, damage_pct: 125 });
    }

    #[test]
    fn breakthrough_phase_table() {
        use CombatPhase::*;
        use CombatTactic as T;
        assert_eq!(next_breakthrough_phase(Default, 0, T::Breakthrough, false), (Breakthrough, 24));
        assert_eq!(next_breakthrough_phase(Breakthrough, 3, T::Breakthrough, false), (Breakthrough, 24), "продлён");
        assert_eq!(next_breakthrough_phase(Default, 0, T::Breakthrough, true), (Default, 0), "парирован");
        assert_eq!(next_breakthrough_phase(Breakthrough, 6, T::Assault, false), (Breakthrough, 6), "срок идёт");
        assert_eq!(next_breakthrough_phase(Breakthrough, 0, T::Assault, false), (Default, 0), "срок вышел");
        assert_eq!(next_breakthrough_phase(CloseQuarters, 5, T::Attack, false), (CloseQuarters, 5), "чужую фазу не трогает");
    }

    #[test]
    fn breakthrough_phase_halves_the_defenders_entrenchment_bonus() {
        // окопанность 10 даёт +20% защиты: 225 * 120 / 100 = 270 -> 800 * 90 / 360 = 200
        let attacker = make_neutral(1);
        let mut dug_in = make_neutral(2);
        dug_in.entrenchment = 10;
        assert_eq!(combat_bp_ctx(&attacker, &dug_in, 0, HitContext::NONE), 200);
        // в прорыве окопанность 10 / 2 = 5 -> +10%: защита 247 -> 800 * 90 / 337 = 213
        let cut = HitContext::NONE.with_entrenchment_divisor(2);
        assert_eq!(combat_bp_ctx(&attacker, &dug_in, 0, cut), 213);
        // нечётная окопанность делится вниз: 7 / 2 = 3 (+6%)
        dug_in.entrenchment = 7;
        assert_eq!(combat_bp_ctx(&attacker, &dug_in, 0, cut), 800 * 90 / (90 + 225 * 106 / 100));
        // в бою: те же дивизии, но фаза прорыва — защитник теряет больше организации; таймер идёт по часам
        let result = |phase: CombatPhase| {
            let (mut app, _att, def) = scenario(&[1], 1, 1);
            app.world_mut().get_mut::<Division>(def[0]).expect("защитник").entrenchment = 10;
            set_phase(&mut app, phase, 24);
            tick(&mut app);
            let d = app.world().get::<Division>(def[0]).expect("защитник");
            (d.max_org * 1000 - d.org_milli(), combat_of(&mut app).phase_hours_left)
        };
        let (normal, _) = result(CombatPhase::Default);
        let (cut_loss, hours) = result(CombatPhase::Breakthrough);
        assert!(cut_loss > normal, "{cut_loss} > {normal}");
        assert_eq!(hours, 23, "часовой декремент");
    }

    #[test]
    fn breakthrough_phase_status_text() {
        let (mut app, _a, _d) = scenario(&[1], 1, 1);
        set_phase(&mut app, CombatPhase::Breakthrough, 9);
        assert_eq!(
            phase_text(&combat_of(&mut app)).as_deref(),
            Some("Фаза: Прорыв (осталось 9 ч.) | Окопанность врага урезана на 50%")
        );
    }

    #[test]
    fn planning_scales_effective_attack() {
        let templates = DivisionTemplates::default();
        let make = |id| {
            Division::from_template(
                id,
                "d".into(),
                "GER".into(),
                1,
                PLAIN_INFANTRY_TEMPLATE.to_string(),
                templates.get_or_base(PLAIN_INFANTRY_TEMPLATE),
            )
            .trained()
        };
        let (attacker, defender) = (make(1), make(2));
        // атака 90, защита 225: 800 * 90 / 315 = 228; с +30%: атака 117 -> 800 * 117 / 342 = 273
        assert_eq!(combat_bp(&attacker, &defender, 0), 228);
        assert_eq!(combat_bp(&attacker, &defender, 30), 273);
        // выше MAX_PLANNING не работает
        assert_eq!(combat_bp(&attacker, &defender, 100), 273);
    }
    #[test]
    fn attack_repelled_when_line_breaks() {
        // Слабые атакующие против сильной обороны: линия атаки выбита, провинция остаётся за защитой
        let (mut app, att, _def) = scenario(&[1; 2], 5, 1);
        let mut ticks = 0;
        while app.world_mut().query::<&ActiveCombat>().iter(app.world()).count() > 0 {
            tick(&mut app);
            ticks += 1;
            assert!(ticks < 500, "бой не закончился");
        }
        let world = app.world();
        assert_eq!(world.resource::<ProvinceControllers>().0.get(&2).map(String::as_str), Some("FRA"));
        for &e in &att {
            let d = world.get::<Division>(e).expect("атакующие отступили, а не погибли");
            assert_eq!((d.current_province, d.combat_state), (1, CombatState::None));
            assert!(d.movement.is_none());
        }
    }
}

#[cfg(test)]
mod armor_and_speed_tests {
    use super::*;

    fn division(template: &str) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(1, "d".into(), "GER".into(), 1, template.to_string(), templates.get_or_base(template)).trained()
    }

    #[test]
    fn armor_halves_damage_and_amplifies_return_fire() {
        let infantry = division(BASE_TEMPLATE); // броня 0, пробитие 4
        let panzer = division("Panzer-Division"); // броня 11, пробитие 16
        // пехота не пробивает броню (11 > 4): её удар по танкам вдвое слабее
        assert_eq!(apply_armor_rule(1000, &infantry, &panzer, false), 500);
        // танки отвечают на 50% сильнее (11 > 4)
        assert_eq!(apply_armor_rule(1000, &panzer, &infantry, false), 1500);
        // пехота против пехоты и танки против танков (11 < 16) — без изменений
        assert_eq!(apply_armor_rule(1000, &infantry, &infantry, false), 1000);
        assert_eq!(apply_armor_rule(1000, &panzer, &panzer, false), 1000);
    }

    #[test]
    fn armor_rule_applies_when_both_sides_resist() {
        // Сильная броня у обеих сторон и слабое пробитие: урон и режется, и усиливается (1000 / 2 * 1.5 = 750)
        let mut a = division(BASE_TEMPLATE);
        let mut b = division(BASE_TEMPLATE);
        (a.armor, a.piercing) = (50, 10);
        (b.armor, b.piercing) = (50, 10);
        assert_eq!(apply_armor_rule(1000, &a, &b, false), 750);
    }

    #[test]
    fn march_time_scales_with_speed() {
        // Равнина 24 ч у пехоты (4.0 км/ч); танки 8.0 — вдвое быстрее, моторизация 12.0 — втрое
        assert_eq!(march_hours(Some("plains"), 40), 24);
        assert_eq!(march_hours(Some("plains"), 80), 12);
        assert_eq!(march_hours(Some("plains"), 120), 8);
        // Горы 60 ч -> 30 ч у танков; лес 36 ч -> 12 ч у мотопехоты
        assert_eq!(march_hours(Some("mountain"), 80), 30);
        assert_eq!(march_hours(Some("forest"), 120), 12);
        // Округление вверх (42 * 40 / 120 = 14), нулевая скорость не ломает деление
        assert_eq!(march_hours(Some("marsh"), 120), 14);
        assert!(march_hours(Some("plains"), 0) >= 24);
    }

    #[test]
    fn division_speed_comes_from_template() {
        assert_eq!(division(BASE_TEMPLATE).max_speed, 40);
        assert_eq!(division("Infanterie-Division (mot.)").max_speed, 120);
        assert_eq!(division("Panzer-Division").max_speed, 80);
    }
}

#[cfg(test)]
mod supply_tests {
    use super::*;

    fn spawn_division(app: &mut App, id: u32, owner: &str, province: u32) -> Entity {
        let templates = DivisionTemplates::default();
        let d = Division::from_template(
            id,
            format!("Div {id}"),
            owner.to_string(),
            province,
            BASE_TEMPLATE.to_string(),
            templates.get_or_base(BASE_TEMPLATE),
        );
        app.world_mut().spawn(d).id()
    }

    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    fn division(app: &App, e: Entity) -> &Division {
        app.world().get::<Division>(e).expect("дивизия жива")
    }

    fn network(covered: &[(u32, &str)]) -> SupplyNetwork {
        SupplyNetwork {
            hubs: Default::default(),
            coverage: covered.iter().map(|&(p, o)| (p, o.to_string())).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn new_divisions_start_supplied() {
        let mut app = App::new();
        let e = spawn_division(&mut app, 1, "GER", 1);
        let d = division(&app, e);
        assert!(d.is_supplied);
        assert_eq!(d.attrition_days, 0);
    }

    #[test]
    fn cut_off_division_suffers_attrition_and_no_org_recovery() {
        let mut app = App::new();
        app.add_event::<DailyTickEvent>()
            .insert_resource(network(&[(1, "GER")]))
            .add_systems(Update, daily_supply);

        let supplied = spawn_division(&mut app, 1, "GER", 1); // зона германского хаба
        let cut_off = spawn_division(&mut app, 2, "GER", 9); // вне любой зоны
        let in_enemy_zone = spawn_division(&mut app, 3, "GER", 2); // зона, но чужого хаба
        app.world_mut().get_mut::<Division>(supplied).unwrap().current_org = 30;
        app.world_mut().get_mut::<Division>(cut_off).unwrap().current_org = 30;
        app.world_mut().resource_mut::<SupplyNetwork>().coverage.insert(2, "FRA".to_string());

        day(&mut app);
        // снабжённая отдыхает: +10% от 60 = +6 организации
        let d = division(&app, supplied);
        assert_eq!((d.is_supplied, d.attrition_days, d.current_org), (true, 0, 36));
        // отрезанная: 2% от 900 = 18, организация не растёт
        let d = division(&app, cut_off);
        assert_eq!((d.is_supplied, d.attrition_days), (false, 1));
        assert_eq!((d.current_strength, d.current_org), (882, 30));
        assert_eq!(d.losses, 0, "небоевые потери не засчитываются как боевые");
        assert!(!division(&app, in_enemy_zone).is_supplied, "чужой хаб свою дивизию не снабжает");

        // второй день: 2% от 882 = 17
        day(&mut app);
        let d = division(&app, cut_off);
        assert_eq!((d.attrition_days, d.current_strength, d.current_org), (2, 865, 30));

        // дивизию подвели под зону хаба: статус и счётчик сбрасываются, организация снова растёт
        app.world_mut().resource_mut::<SupplyNetwork>().coverage.insert(9, "GER".to_string());
        day(&mut app);
        let d = division(&app, cut_off);
        assert_eq!((d.is_supplied, d.attrition_days, d.current_strength, d.current_org), (true, 0, 865, 36));
    }

    #[test]
    fn attrition_never_kills_and_marching_units_do_not_rest() {
        let mut app = App::new();
        app.add_event::<DailyTickEvent>().insert_resource(network(&[(1, "GER")])).add_systems(Update, daily_supply);

        let starving = spawn_division(&mut app, 1, "GER", 9);
        app.world_mut().get_mut::<Division>(starving).unwrap().current_strength = 1;
        let marching = spawn_division(&mut app, 2, "GER", 1);
        {
            let mut d = app.world_mut().get_mut::<Division>(marching).unwrap();
            d.current_org = 10;
            d.movement = Some(MovementProgress { from: 1, to: 2, hours_left: 5, total_hours: 24 });
        }
        for _ in 0..3 {
            day(&mut app);
        }
        assert_eq!(division(&app, starving).current_strength, 1, "прочность не падает ниже 1");
        assert_eq!(division(&app, starving).attrition_days, 3);
        assert_eq!(division(&app, marching).current_org, 10, "на марше организация не восстанавливается");
    }
}

#[cfg(test)]
mod terrain_tests {
    use super::*;

    fn division(template: &str) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(1, "d".into(), "GER".into(), 1, template.to_string(), templates.get_or_base(template)).trained()
    }

    #[test]
    fn terrain_table_matches_the_rules() {
        let m = |t| terrain_modifiers(Some(t));
        // (бонус защиты, штраф атаки, доп. штраф бронированным)
        let parts = |t: TerrainModifiers| (t.defense_bonus, t.attack_penalty, t.armored_attack_penalty);
        assert_eq!(parts(m("plains")), (0, 0, 0));
        assert_eq!(parts(m("forest")), (20, 10, 0));
        assert_eq!(parts(m("hills")), (25, 15, 0));
        assert_eq!(parts(m("mountain")), (50, 30, 20));
        assert_eq!(parts(m("marsh")), (20, 25, 30));
        assert_eq!(parts(m("urban")), (30, 30, 20));
        // прочие типы и неизвестная местность — без модификаторов
        assert_eq!(parts(m("desert")), (0, 0, 0));
        assert_eq!(parts(terrain_modifiers(None)), (0, 0, 0));
    }

    #[test]
    fn armored_units_pay_extra_in_mountains_and_marsh() {
        let infantry = division(BASE_TEMPLATE); // твёрдость 0
        let panzer = division("Panzer-Division"); // твёрдость 55 > 50
        assert!(panzer.hardness > 50 && infantry.hardness <= 50);
        let mountain = terrain_modifiers(Some("mountain"));
        let marsh = terrain_modifiers(Some("marsh"));
        assert_eq!(HitContext::for_attack(&mountain, &infantry, false).attack_penalty, 30);
        assert_eq!(HitContext::for_attack(&mountain, &panzer, false).attack_penalty, 50);
        assert_eq!(HitContext::for_attack(&marsh, &infantry, false).attack_penalty, 25);
        assert_eq!(HitContext::for_attack(&marsh, &panzer, false).attack_penalty, 55);
        // лес броню не штрафует; защитный бонус всегда берётся из рельефа
        let forest = terrain_modifiers(Some("forest"));
        assert_eq!(HitContext::for_attack(&forest, &panzer, false), HitContext { attack_penalty: 10, defense_bonus: 20, ..HitContext::NONE });
        // река добавляет -30% поверх рельефа
        assert_eq!(HitContext::for_attack(&mountain, &panzer, true).attack_penalty, 80);
    }

    #[test]
    fn terrain_and_river_scale_attack_and_defense() {
        let (attacker, defender) = (division(PLAIN_INFANTRY_TEMPLATE), division(PLAIN_INFANTRY_TEMPLATE));
        // база: атака 90, защита 225 -> 800 * 90 / 315 = 228
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, HitContext::NONE), 228);
        // лес: атака 90 * 90 / 100 = 81, защита 225 * 120 / 100 = 270 -> 800 * 81 / 351 = 184
        let forest = HitContext { attack_penalty: 10, defense_bonus: 20, ..HitContext::NONE };
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, forest), 184);
        // горы: атака 63, защита 337 -> 800 * 63 / 400 = 126
        let mountain = HitContext { attack_penalty: 30, defense_bonus: 50, ..HitContext::NONE };
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, mountain), 126);
        // только река: атака 63, защита 225 -> 800 * 63 / 288 = 175
        let river = HitContext { attack_penalty: RIVER_PENALTY_PCT, defense_bonus: 0, ..HitContext::NONE };
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, river), 175);
        // штрафы свыше 100% не уводят атаку в отрицательные значения (остаётся минимальный урон)
        let absurd = HitContext { attack_penalty: 250, defense_bonus: 0, ..HitContext::NONE };
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, absurd), 1);
    }

    #[test]
    fn combat_label_shows_terrain_modifiers() {
        assert_eq!(terrain_label(Some("forest"), None), "Местность: forest (Защ: +20%, Атк: -10%)");
        assert_eq!(terrain_label(Some("plains"), None), "Местность: plains (Защ: +0%, Атк: -0%)");
        assert_eq!(terrain_label(Some("hills"), Some(RIVER_PENALTY_PCT)), "Местность: hills (Защ: +25%, Атк: -45%) [река]");
        // с сапёрами река штрафует слабее: 15 + 10
        assert_eq!(terrain_label(Some("hills"), Some(10)), "Местность: hills (Защ: +25%, Атк: -25%) [река]");
        assert_eq!(terrain_label(None, None), "Местность: unknown (Защ: +0%, Атк: -0%)");
    }

    /// Реки из `map/rivers.bmp` на реальных данных (пропускается без игры).
    #[test]
    fn real_data_river_borders() {
        use std::path::Path;
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let map = crate::map::load_province_map(root).expect("карта");
        let (_, adjacency, rivers) = crate::map::build_id_texture_and_adjacency(&map);
        let edges: usize = adjacency.neighbors.values().map(|s| s.len()).sum::<usize>() / 2;
        println!("границ с рекой: {} из {edges}", rivers.0.len());
        assert!(rivers.0.len() > 100, "реки должны находиться на сотнях границ");
        assert!(rivers.0.len() < edges / 2, "река не может быть почти на каждой границе");
        // каждая речная граница — реальная пара соседей
        assert!(rivers.0.iter().all(|&(a, b)| adjacency.neighbors.get(&a).is_some_and(|s| s.contains(&b))));
        assert!(rivers.crosses(rivers.0.iter().next().unwrap().1, rivers.0.iter().next().unwrap().0));
    }
}

#[cfg(test)]
mod recon_air_tests {
    use super::*;

    fn division(template: &str) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(1, "d".into(), "GER".into(), 1, template.to_string(), templates.get_or_base(template)).trained()
    }

    #[test]
    fn division_recon_comes_from_its_battalions() {
        assert_eq!(division(BASE_TEMPLATE).recon, 1);
        assert_eq!(division("Infanterie-Division (mot.)").recon, 3);
        assert_eq!(division("Panzer-Division").recon, 3); // максимум: моторизованные 3 > лёгкие танки 2
    }

    #[test]
    fn recon_advantage_compares_average_recon() {
        assert_eq!(recon_advantage(&[3, 3], &[1, 1]), Some(true));
        assert_eq!(recon_advantage(&[1], &[2]), Some(false));
        assert_eq!(recon_advantage(&[2, 2], &[2]), None, "равные средние — преимущества нет");
        // дробные средние не теряются: (1+2)/2 = 1.5 против 1
        assert_eq!(recon_advantage(&[1, 2], &[1]), Some(true));
        // число дивизий не важно, важна средняя: 4 слабых против 1 сильной
        assert_eq!(recon_advantage(&[1, 1, 1, 1], &[3]), Some(false));
        // пустая линия — не сравниваем
        assert_eq!(recon_advantage(&[], &[3]), None);
        assert_eq!(recon_advantage(&[3], &[]), None);
    }

    #[test]
    fn recon_advantage_doubles_reserve_chance() {
        // шанс 2..=5% по id; с преимуществом в разведке — вдвое выше
        assert_eq!(reserve_chance_pct(4, false), 2);
        assert_eq!(reserve_chance_pct(4, true), 4);
        assert_eq!(reserve_chance_pct(7, false), 5);
        assert_eq!(reserve_chance_pct(7, true), 10);
    }

    #[test]
    fn air_bonus_and_penalty_enter_the_damage_formula() {
        let (attacker, defender) = (division(PLAIN_INFANTRY_TEMPLATE), division(PLAIN_INFANTRY_TEMPLATE));
        // база: 800 * 90 / 315 = 228
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, HitContext::NONE), 228);
        // перевес 40: +4% атаки -> 93, 800 * 93 / 318 = 233
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, HitContext::NONE.with_air(4, 0)), 233);
        // давление врага: -6% защиты -> 211, 800 * 90 / 301 = 239
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, HitContext::NONE.with_air(0, 6)), 239);
        // вместе: 800 * 93 / 304 = 244
        assert_eq!(combat_bp_ctx(&attacker, &defender, 0, HitContext::NONE.with_air(4, 6)), 244);
        // штраф защиты не уходит ниже нуля
        assert!(combat_bp_ctx(&attacker, &defender, 0, HitContext::NONE.with_air(0, 500)) >= 228);
    }

    #[test]
    fn combat_plate_shows_recon_and_sky() {
        let side = |recon_sum, active| SideStats { used_width: 0, reserves: 0, org: 0, org_max: 0, strength: 0, strength_max: 0, recon_sum, active };
        assert_eq!(recon_text(3, 1), "3.0");
        assert_eq!(recon_text(5, 2), "2.5");
        assert_eq!(recon_text(0, 0), "0.0");
        let sky = |region: Option<&str>, edge: i32| air_line(region, edge, "GER", "FRA");
        assert_eq!(
            recon_air_label(&side(9, 3), &side(4, 4), &sky(Some("Западная Германия"), 40)),
            "Разведка: 3.0 vs 1.0 | Небо (Западная Германия): Германия +40"
        );
        assert_eq!(recon_air_label(&side(1, 1), &side(1, 1), &sky(None, -15)), "Разведка: 1.0 vs 1.0 | Небо: Франция +15");
        assert_eq!(recon_air_label(&side(1, 1), &side(1, 1), &sky(None, 0)), "Разведка: 1.0 vs 1.0 | Небо: паритет");
    }
}

#[cfg(test)]
mod support_tests {
    use super::*;

    fn division(template: &str) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(1, "d".into(), "GER".into(), 1, template.to_string(), templates.get_or_base(template)).trained()
    }

    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    #[test]
    fn standard_division_carries_engineers_and_artillery() {
        let d = division(BASE_TEMPLATE);
        assert_eq!(d.support, vec!["Сапёры", "Арт"]);
        assert_eq!((d.max_entrenchment, d.river_penalty_reduction, d.hard_attack), (15, 20, 2));
        assert_eq!(d.battalion_count, 9, "роты обеспечения не считаются батальонами");
        let plain = division(PLAIN_INFANTRY_TEMPLATE);
        assert!(plain.support.is_empty());
        assert_eq!((plain.max_entrenchment, plain.river_penalty_reduction), (10, 0));
        // поддержка не занимает ширину фронта
        assert_eq!(d.combat_width, plain.combat_width);
        assert_eq!(d.combat_width, 18);
    }

    #[test]
    fn engineers_raise_the_entrenchment_cap_to_fifteen() {
        let mut app = App::new();
        app.add_event::<DailyTickEvent>().add_systems(Update, daily_entrenchment);
        let engineers = app.world_mut().spawn(division(BASE_TEMPLATE)).id();
        let plain = app.world_mut().spawn(division(PLAIN_INFANTRY_TEMPLATE)).id();
        for _ in 0..20 {
            day(&mut app);
        }
        assert_eq!(app.world().get::<Division>(engineers).unwrap().entrenchment, 15);
        assert_eq!(app.world().get::<Division>(plain).unwrap().entrenchment, 10);
    }

    #[test]
    fn fifteen_entrenchment_gives_thirty_percent_defense_and_cap_is_enforced_in_combat() {
        let attacker = division(BASE_TEMPLATE); // атака 90 + 25 артиллерии = 115
        assert_eq!(attacker.soft_attack, 115);

        let mut dug_in = division(BASE_TEMPLATE);
        dug_in.entrenchment = 15; // 225 * 130 / 100 = 292 -> 800 * 115 / 407 = 226
        assert_eq!(combat_bp_ctx(&attacker, &dug_in, 0, HitContext::NONE), 226);

        // у дивизии без сапёров 15 пунктов упираются в предел 10 (+20%): 225 * 120 / 100 = 270 -> 800 * 115 / 385 = 238
        let mut plain = division(PLAIN_INFANTRY_TEMPLATE);
        plain.entrenchment = 15;
        assert_eq!(combat_bp_ctx(&attacker, &plain, 0, HitContext::NONE), 238);

        // сапёры в обороне дают меньше урона по себе, чем при пределе 10
        assert!(combat_bp_ctx(&attacker, &dug_in, 0, HitContext::NONE) < combat_bp_ctx(&attacker, &plain, 0, HitContext::NONE));
    }

    #[test]
    fn engineers_cut_the_river_penalty_from_thirty_to_ten() {
        let (sappers, plain) = (division(BASE_TEMPLATE), division(PLAIN_INFANTRY_TEMPLATE));
        assert_eq!(river_penalty_for(&sappers), 10);
        assert_eq!(river_penalty_for(&plain), 30);

        let flat = terrain_modifiers(Some("plains"));
        assert_eq!(HitContext::for_attack(&flat, &sappers, true).attack_penalty, 10);
        assert_eq!(HitContext::for_attack(&flat, &plain, true).attack_penalty, 30);
        // без реки штрафа нет ни у кого
        assert_eq!(HitContext::for_attack(&flat, &sappers, false).attack_penalty, 0);
        // рельеф складывается с рекой: горы -30% и река -10% с сапёрами = -40%
        let mountain = terrain_modifiers(Some("mountain"));
        assert_eq!(HitContext::for_attack(&mountain, &sappers, true).attack_penalty, 40);

        // сила удара: атака 115 * 90 / 100 = 103 -> 800 * 103 / 328 = 251 (с обычной рекой было бы 115 * 70 / 100 = 80 -> 800 * 80 / 305 = 209)
        let target = division(PLAIN_INFANTRY_TEMPLATE);
        assert_eq!(combat_bp_ctx(&sappers, &target, 0, HitContext::for_attack(&flat, &sappers, true)), 251);
        let mut no_engineers = sappers.clone();
        no_engineers.river_penalty_reduction = 0;
        assert_eq!(combat_bp_ctx(&no_engineers, &target, 0, HitContext::for_attack(&flat, &no_engineers, true)), 209);
    }

    #[test]
    fn artillery_company_adds_attack_without_using_front_width() {
        let standard = division(BASE_TEMPLATE);
        let plain = division(PLAIN_INFANTRY_TEMPLATE);
        let target = division(PLAIN_INFANTRY_TEMPLATE);
        // прямая прибавка к атаке: 115 против 90 -> удар сильнее (270 против 228)
        assert_eq!(combat_bp(&plain, &target, 0), 228);
        assert_eq!(combat_bp(&standard, &target, 0), 800 * 115 / (115 + 225));
        assert!(combat_bp(&standard, &target, 0) > combat_bp(&plain, &target, 0));
        // а ширина прежняя: 18 на дивизию, то есть на линию 80 по-прежнему помещается 4 дивизии
        assert_eq!(standard.combat_width, plain.combat_width);
    }

    #[test]
    fn support_label_line_for_the_panel() {
        let line = |d: &Division| {
            if d.support.is_empty() { "Поддержка: нет".to_string() } else { format!("Поддержка: [{}]", d.support.join(", ")) }
        };
        assert_eq!(line(&division(BASE_TEMPLATE)), "Поддержка: [Сапёры, Арт]");
        assert_eq!(line(&division(PLAIN_INFANTRY_TEMPLATE)), "Поддержка: нет");
    }
}

#[cfg(test)]
mod hardness_tests {
    use super::*;
    use crate::units::BattalionDef;

    fn division_of(template: &DivisionTemplate) -> Division {
        Division::from_template(1, "d".into(), "GER".into(), 1, template.name.clone(), template).trained()
    }

    fn registered(name: &str) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(1, "d".into(), "GER".into(), 1, name.to_string(), templates.get_or_base(name)).trained()
    }

    fn custom(name: &str, battalions: Vec<BattalionDef>) -> Division {
        division_of(&DivisionTemplate { name: name.to_string(), battalions })
    }

    #[test]
    fn stats_keep_soft_and_hard_attack_apart() {
        // пехота: мягкая атака преобладает, жёсткой нет
        let infantry = registered(PLAIN_INFANTRY_TEMPLATE);
        assert_eq!((infantry.soft_attack, infantry.hard_attack), (90, 0));
        // танки: жёсткая атака заметно выше, чем у пехоты (4 лёгких танка по 10 + моторизованные 0)
        let panzer = registered("Panzer-Division");
        assert_eq!((panzer.soft_attack, panzer.hard_attack), (68, 40));
        // артиллерия поддержки: 25 мягкой и 2 жёсткой
        assert_eq!(registered(BASE_TEMPLATE).hard_attack, 2);
        // подпись в панели
        assert_eq!(attack_label(&infantry), "Атака: 90 мягк. / 0 твёрд. | Твёрдость: 0%");
        assert_eq!(attack_label(&panzer), "Атака: 68 мягк. / 40 твёрд. | Твёрдость: 55%");
    }

    #[test]
    fn infantry_against_infantry_uses_full_soft_attack() {
        let (attacker, target) = (registered(PLAIN_INFANTRY_TEMPLATE), registered(PLAIN_INFANTRY_TEMPLATE));
        assert_eq!(target.hardness, 0);
        assert_eq!(blended_attack(&attacker, target.hardness), 90, "100% мягкой атаки");
        // формула удара даёт прежние 800 * 90 / 315 = 228
        assert_eq!(combat_bp(&attacker, &target, 0), 228);
    }

    #[test]
    fn infantry_against_armor_is_cut_by_target_hardness() {
        let infantry = registered(PLAIN_INFANTRY_TEMPLATE);
        let panzer = registered("Panzer-Division");
        // твёрдость танковой дивизии 55%: (90 * 45 + 0 * 55) / 100 = 40
        assert_eq!(blended_attack(&infantry, panzer.hardness), 40);
        // полностью бронированная цель (100%): мягкая атака не работает совсем
        assert_eq!(blended_attack(&infantry, 100), 0);

        // цель той же защиты 225, но твёрдостью 100: остаётся только минимальный урон (1 bp)
        let mut tank_like = registered(PLAIN_INFANTRY_TEMPLATE);
        tank_like.hardness = 100;
        assert_eq!(combat_bp(&infantry, &tank_like, 0), 1);
        // при твёрдости 55 удар слабее, чем по живой силе: 800 * 40 / 265 = 120 против 228
        tank_like.hardness = 55;
        assert_eq!(combat_bp(&infantry, &tank_like, 0), 120);
    }

    #[test]
    fn tanks_and_heavy_artillery_hit_armor_with_hard_attack() {
        let infantry = registered(PLAIN_INFANTRY_TEMPLATE);
        // 6 средних танков: мягкая 120, жёсткая 144
        let tanks = custom("tanks", vec![BattalionDef::medium_tank(); 6]);
        assert_eq!((tanks.soft_attack, tanks.hard_attack), (120, 144));
        // по цели с твёрдостью 90: (120 * 10 + 144 * 90) / 100 = 141, а пехота: (90 * 10 + 0) / 100 = 9
        assert_eq!(blended_attack(&tanks, 90), 141);
        assert_eq!(blended_attack(&infantry, 90), 9);

        // та же защита 225, твёрдость 90: 800 * 141 / 366 = 308 против 800 * 9 / 234 = 30
        let mut armored = registered(PLAIN_INFANTRY_TEMPLATE);
        armored.hardness = 90;
        assert_eq!(combat_bp(&tanks, &armored, 0), 308);
        assert_eq!(combat_bp(&infantry, &armored, 0), 30);

        // тяжёлая артиллерия (тестовая рота с жёсткой атакой 40): по бронетехнике сильнее обычной (2)
        let heavy = BattalionDef { hard_attack: 40, ..BattalionDef::support_artillery() };
        let mut battalions = vec![BattalionDef::infantry(); 9];
        battalions.push(heavy);
        let heavy_division = custom("heavy-arty", battalions);
        // (115 * 10 + 40 * 90) / 100 = 47 против обычной артиллерии (115 * 10 + 2 * 90) / 100 = 13
        assert_eq!(blended_attack(&heavy_division, 90), 47);
        assert_eq!(blended_attack(&registered(BASE_TEMPLATE), 90), 13);
        // по живой силе (твёрдость 0) обе одинаково используют мягкую атаку 115
        assert_eq!(blended_attack(&heavy_division, 0), 115);
    }

    #[test]
    fn blend_divides_once_and_clamps_hardness() {
        let mut d = registered(PLAIN_INFANTRY_TEMPLATE);
        // мягкая 1 и жёсткая 1 при твёрдости 50: (1 * 50 + 1 * 50) / 100 = 1; раздельное деление дало бы 0 + 0
        (d.soft_attack, d.hard_attack) = (1, 1);
        assert_eq!(blended_attack(&d, 50), 1);
        // твёрдость больше 100 не уводит мягкую долю в минус и равна 100
        (d.soft_attack, d.hard_attack) = (90, 30);
        assert_eq!(blended_attack(&d, 250), blended_attack(&d, 100));
        assert_eq!(blended_attack(&d, 100), 30);
        // нулевая атака и нулевая прочность не вызывают деления на ноль
        (d.soft_attack, d.hard_attack, d.max_strength) = (0, 0, 0);
        assert_eq!(combat_bp(&d, &registered(PLAIN_INFANTRY_TEMPLATE), 0), 1);
    }

    #[test]
    fn blended_attack_feeds_the_existing_multipliers() {
        // база берётся из смеси, дальше работают прежние множители: планирование +30% на атаке 40 даёт 52
        let infantry = registered(PLAIN_INFANTRY_TEMPLATE);
        let mut armored = registered(PLAIN_INFANTRY_TEMPLATE);
        armored.hardness = 55;
        // атака 40: 800 * 40 / 265 = 120; с планированием 30%: 40 * 130 / 100 = 52 -> 800 * 52 / 277 = 150
        assert_eq!(combat_bp(&infantry, &armored, 0), 120);
        assert_eq!(combat_bp(&infantry, &armored, 30), 150);
    }
}

#[cfg(test)]
mod refit_tests {
    use super::*;
    use crate::units::BattalionDef;

    fn registered(name: &str) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(1, "d".into(), "GER".into(), 1, name.to_string(), templates.get_or_base(name)).trained()
    }

    #[test]
    fn refit_applies_new_stats_resets_entrenchment_and_halves_org() {
        let templates = DivisionTemplates::default();
        let panzer = templates.get("Panzer-Division").unwrap();

        let mut d = registered(BASE_TEMPLATE);
        d.entrenchment = 12;
        d.current_org = 50;
        d.current_strength = 450; // половина от 900
        d.org_frac = 300;
        d.losses = 77;
        d.planning = 20;
        d.path = vec![5, 6];
        d.refit(panzer);

        // все характеристики пришли из calculate_stats() нового шаблона
        let stats = panzer.calculate_stats();
        assert_eq!(d.template_name, "Panzer-Division");
        assert_eq!(d.battalion_count, 6);
        assert_eq!((d.max_org, d.max_strength, d.combat_width, d.max_speed), (stats.max_org, 600, 12, 80));
        assert_eq!((d.soft_attack, d.hard_attack, d.defense), (stats.soft_attack, stats.hard_attack, stats.defense));
        assert_eq!((d.armor, d.piercing, d.hardness, d.recon), (stats.armor, stats.piercing, 55, 3));
        assert_eq!(d.max_manpower, stats.total_manpower);
        assert_eq!(d.equipment_needs, stats.equipment_needs);
        // поддержки у танковой дивизии нет: окопанность до 10, реки без сапёров
        assert!(d.support.is_empty());
        assert_eq!((d.max_entrenchment, d.river_penalty_reduction), (10, 0));

        // реорганизация: окопанность 0, организация вдвое меньше (50 -> 25), дробный остаток сброшен
        assert_eq!((d.entrenchment, d.current_org, d.org_frac), (0, 25, 0));
        // прочность сохраняет долю от максимума: 450 / 900 -> 300 / 600
        assert_eq!(d.current_strength, 300);
        // потери и планирование не пропадают, отложенный маршрут снят
        assert_eq!((d.losses, d.planning), (77, 20));
        assert!(d.path.is_empty());
    }

    #[test]
    fn refit_round_trip_restores_template_stats() {
        let templates = DivisionTemplates::default();
        let mut d = registered(BASE_TEMPLATE);
        d.refit(templates.get("Panzer-Division").unwrap());
        d.refit(templates.get(BASE_TEMPLATE).unwrap());
        let fresh = registered(BASE_TEMPLATE);
        assert_eq!(
            (d.max_org, d.max_strength, d.soft_attack, d.hard_attack, d.combat_width, d.max_speed, d.support.clone()),
            (fresh.max_org, fresh.max_strength, fresh.soft_attack, fresh.hard_attack, fresh.combat_width, fresh.max_speed, fresh.support.clone())
        );
        assert_eq!((d.max_entrenchment, d.river_penalty_reduction), (15, 20), "сапёры вернулись");
        // но организация дважды урезана пополам: 60 -> 30 -> 15
        assert_eq!(d.current_org, 15);
    }

    #[test]
    fn refit_clamps_org_to_the_new_maximum_and_keeps_empty_divisions_empty() {
        let weak_org = DivisionTemplate {
            name: "low-org".into(),
            battalions: vec![BattalionDef { max_org: 20, ..BattalionDef::infantry() }; 9],
        };
        let mut d = registered(PLAIN_INFANTRY_TEMPLATE);
        assert_eq!(d.current_org, 60);
        d.refit(&weak_org);
        assert_eq!((d.max_org, d.current_org), (20, 20), "60 / 2 = 30, но новый максимум 20");

        let mut empty = registered(BASE_TEMPLATE);
        empty.current_strength = 0;
        empty.refit(&weak_org);
        assert_eq!(empty.current_strength, 0, "пустая дивизия остаётся пустой");
        // непустая не обнуляется из-за округления
        let mut tiny = registered(BASE_TEMPLATE);
        tiny.current_strength = 1;
        tiny.refit(&weak_org);
        assert_eq!(tiny.current_strength, 1);
    }

    fn app_with(divisions: &[(&str, bool)]) -> (App, Vec<Entity>) {
        let mut app = App::new();
        app.add_event::<RefitDivision>()
            .init_resource::<DivisionTemplates>()
            .add_systems(Update, apply_refits);
        let ids = divisions
            .iter()
            .enumerate()
            .map(|(i, &(owner, marching))| {
                let mut d = registered(BASE_TEMPLATE);
                d.id = i as u32 + 1;
                d.owner = owner.to_string();
                d.entrenchment = 7;
                if marching {
                    d.movement = Some(MovementProgress { from: 1, to: 2, hours_left: 5, total_hours: 24 });
                }
                app.world_mut().spawn(d).id()
            })
            .collect();
        (app, ids)
    }

    fn order(app: &mut App, division: Entity, template: &str) {
        app.world_mut().send_event(RefitDivision { division, template: template.to_string() });
        app.update();
    }

    fn template_of(app: &App, e: Entity) -> String {
        app.world().get::<Division>(e).unwrap().template_name.clone()
    }

    #[test]
    fn refit_system_validates_nation_template_and_state() {
        let (mut app, ids) = app_with(&[("GER", false), ("FRA", false), ("GER", true), ("GER", false)]);
        // 1. Германия переводит стоящую дивизию в танковую
        order(&mut app, ids[0], "Panzer-Division");
        assert_eq!(template_of(&app, ids[0]), "Panzer-Division");
        assert_eq!(app.world().get::<Division>(ids[0]).unwrap().entrenchment, 0);
        // 2. у Франции такого шаблона нет
        order(&mut app, ids[1], "Panzer-Division");
        assert_eq!(template_of(&app, ids[1]), BASE_TEMPLATE);
        assert_eq!(app.world().get::<Division>(ids[1]).unwrap().entrenchment, 7, "отклонённый приказ ничего не меняет");
        // 3. на марше сменить шаблон нельзя
        order(&mut app, ids[2], "Panzer-Division");
        assert_eq!(template_of(&app, ids[2]), BASE_TEMPLATE);
        // 4. неизвестный шаблон и тестовый шаблон вне списка нации отклоняются
        order(&mut app, ids[3], "Нет такого");
        order(&mut app, ids[3], PLAIN_INFANTRY_TEMPLATE);
        assert_eq!(template_of(&app, ids[3]), BASE_TEMPLATE);
        // французам доступна их моторизованная дивизия (скорость 12.0 км/ч, роты поддержки не тормозят)
        order(&mut app, ids[1], "Division d'Infanterie Motorisée");
        assert_eq!(template_of(&app, ids[1]), "Division d'Infanterie Motorisée");
        assert_eq!(app.world().get::<Division>(ids[1]).unwrap().max_speed, 120);
    }

    #[test]
    fn refit_system_rejects_divisions_in_combat() {
        let (mut app, ids) = app_with(&[("GER", false), ("FRA", false)]);
        app.world_mut().spawn(ActiveCombat {
            province_id: 2,
            attacker_owner: "GER".into(),
            defender_owner: "FRA".into(),
            attackers: vec![ids[0]],
            defenders: vec![ids[1]],
            max_width: 80,
            combat_progress: 0,
            elapsed_hours: 0,
            attacker_tactic: CombatTactic::Attack,
            defender_tactic: CombatTactic::Defend,
            is_attacker_countered: false,
            is_defender_countered: false,
            tactic_change_day: 0,
            phase: CombatPhase::Default,
            phase_hours_left: 0,
            bridge_held: false,
        });
        order(&mut app, ids[0], "Panzer-Division");
        assert_eq!(template_of(&app, ids[0]), BASE_TEMPLATE, "в бою шаблон не меняется");
    }
}

#[cfg(test)]
mod equipment_status_tests {
    use super::*;

    fn registered(name: &str) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(1, "d".into(), "GER".into(), 1, name.to_string(), templates.get_or_base(name)).trained()
    }

    #[test]
    fn division_keeps_needs_by_type_from_its_template() {
        let standard = registered(BASE_TEMPLATE);
        assert_eq!(
            standard.equipment_needs,
            BTreeMap::from([
                (EquipmentType::InfantryEquipment, 900),
                (EquipmentType::SupportEquipment, 10),
                (EquipmentType::Artillery, 24),
            ])
        );
        assert_eq!(standard.required_equipment(), 934);
        let panzer = registered("Panzer-Division");
        assert_eq!(panzer.equipment_needs.get(&EquipmentType::LightTank), Some(&200));
        assert_eq!(panzer.equipment_needs.get(&EquipmentType::InfantryEquipment), Some(&200));
    }

    #[test]
    fn equipment_scales_with_strength_and_status_text_lists_types() {
        let mut d = registered(BASE_TEMPLATE);
        assert_eq!(d.current_equipment(), 934);
        assert_eq!(
            equipment_status_text(&d),
            "Комплектность: Винтовки 900/900 | Оснащение 10/10 | Пушки 24/24"
        );
        // половина прочности — половина снаряжения каждого типа
        d.current_strength = 450;
        assert_eq!(d.equipment_of(EquipmentType::InfantryEquipment), 450);
        assert_eq!(d.current_equipment(), 450 + 5 + 12);
        assert_eq!(
            equipment_status_text(&d),
            "Комплектность: Винтовки 450/900 | Оснащение 5/10 | Пушки 12/24"
        );
    }

    #[test]
    fn status_text_reports_shortage_and_empty_needs() {
        let mut panzer = registered("Panzer-Division");
        panzer.current_strength = 300;
        panzer.equipment_shortage.insert(EquipmentType::LightTank);
        assert_eq!(
            equipment_status_text(&panzer),
            "Комплектность: Винтовки 100/200 | Лёгкие танки 100/200 | Дефицит: Лёгкие танки"
        );
        let mut none = registered(BASE_TEMPLATE);
        none.equipment_needs.clear();
        assert_eq!(equipment_status_text(&none), "Комплектность: не требуется");
        assert_eq!((none.current_equipment(), none.required_equipment()), (0, 0));
    }

    #[test]
    fn refit_replaces_needs_and_clears_shortage() {
        let templates = DivisionTemplates::default();
        let mut d = registered(BASE_TEMPLATE);
        d.equipment_shortage.insert(EquipmentType::Artillery);
        d.refit(templates.get("Panzer-Division").unwrap());
        assert!(d.equipment_shortage.is_empty(), "после реорганизации прежний дефицит неактуален");
        assert_eq!(d.equipment_needs.get(&EquipmentType::LightTank), Some(&200));
        assert!(!d.equipment_needs.contains_key(&EquipmentType::Artillery));
        assert_eq!(d.required_equipment(), 400);
    }
}

#[cfg(test)]
mod stack_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn division(id: u32, owner: &str, province: u32, org_pct: u32, moving: bool) -> Division {
        let templates = DivisionTemplates::default();
        let mut d = Division::from_template(
            id,
            format!("Infanterie-Division {id}"),
            owner.to_string(),
            province,
            BASE_TEMPLATE.to_string(),
            templates.get_or_base(BASE_TEMPLATE),
        );
        d.current_org = d.max_org * org_pct / 100;
        if moving {
            d.movement = Some(MovementProgress { from: province, to: province + 1, hours_left: 10, total_hours: 24 });
        }
        d
    }

    fn spawn(app: &mut App, d: Division) -> Entity {
        app.world_mut()
            .spawn((d, Visibility::default(), Transform::default()))
            .with_children(|p| {
                p.spawn((Sprite::default(), OrgBar));
                p.spawn((Sprite::default(), StrengthBar));
            })
            .id()
    }

    fn visibility(app: &App, e: Entity) -> Visibility {
        *app.world().get::<Visibility>(e).expect("видимость")
    }

    fn bar_width(app: &mut App, parent: Entity, org: bool) -> f32 {
        let mut query = app.world_mut().query::<(&Parent, &Sprite, Has<OrgBar>)>();
        query
            .iter(app.world())
            .find(|(p, _, is_org)| p.get() == parent && *is_org == org)
            .and_then(|(_, s, _)| s.custom_size)
            .map_or(-1.0, |size| size.x)
    }

    fn visuals_app() -> App {
        let mut app = App::new();
        app.init_resource::<SelectedDivision>().init_resource::<StackView>().add_systems(Update, update_stack_visuals);
        app
    }

    #[test]
    fn layout_groups_by_province_and_picks_lowest_id_as_head() {
        let mut world = World::new();
        let (a, b, c, d) = (world.spawn_empty().id(), world.spawn_empty().id(), world.spawn_empty().id(), world.spawn_empty().id());
        let layout = stack_layout([(a, 7, 1), (b, 3, 1), (c, 5, 1), (d, 9, 2)], None);
        assert_eq!(layout.len(), 2);
        assert_eq!(layout[&1].head, b, "наименьший id = 3");
        assert_eq!(layout[&1].members.iter().map(|m| m.0).collect::<Vec<_>>(), vec![3, 5, 7]);
        assert_eq!(layout[&2].members.len(), 1);
        // выбранная дивизия стека становится главной фишкой; выбранная из другой провинции — нет
        assert_eq!(stack_layout([(a, 7, 1), (b, 3, 1)], Some(a))[&1].head, a);
        assert_eq!(stack_layout([(a, 7, 1), (b, 3, 1)], Some(d))[&1].head, b);
        assert!(stack_layout([], None).is_empty());
    }

    #[test]
    fn percent_and_average_are_integer() {
        assert_eq!(percent(95, 100), 95);
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(5, 0), 0);
        assert_eq!(percent(200, 100), 100);
        assert_eq!(average_percent(&[100, 50, 0]), 50);
        assert_eq!(average_percent(&[100, 95]), 97);
        assert_eq!(average_percent(&[]), 0);
    }

    #[test]
    fn badge_and_inspector_row_texts() {
        assert_eq!(stack_badge_text(3), "[3]");
        let d = division(1, "GER", 5, 95, false);
        assert_eq!(stack_row_text(2, &d), "2. Infanterie-Division 1 | Орг: 95% | HP: 100%");
    }

    #[test]
    fn only_one_token_is_visible_per_stack_and_marching_divisions_stay_visible() {
        let mut app = visuals_app();
        let e1 = spawn(&mut app, division(1, "GER", 5, 100, false));
        let e2 = spawn(&mut app, division(2, "GER", 5, 100, false));
        let e3 = spawn(&mut app, division(3, "GER", 5, 100, false));
        let marching = spawn(&mut app, division(4, "GER", 5, 100, true));
        let solo = spawn(&mut app, division(5, "GER", 6, 100, false));
        app.update();
        assert_eq!(visibility(&app, e1), Visibility::Inherited);
        assert_eq!(visibility(&app, e2), Visibility::Hidden);
        assert_eq!(visibility(&app, e3), Visibility::Hidden);
        assert_eq!(visibility(&app, marching), Visibility::Inherited, "марш рисуется отдельно");
        assert_eq!(visibility(&app, solo), Visibility::Inherited);
        assert_eq!(app.world().resource::<StackView>().0[&5].members.len(), 3, "марширующая в стек не входит");
        // выбранная дивизия берёт главную фишку на себя
        app.world_mut().resource_mut::<SelectedDivision>().0 = Some(e3);
        app.update();
        assert_eq!(visibility(&app, e3), Visibility::Inherited);
        assert_eq!(visibility(&app, e1), Visibility::Hidden);
    }

    #[test]
    fn stack_bars_show_the_arithmetic_mean_of_all_divisions() {
        let mut app = visuals_app();
        let e1 = spawn(&mut app, division(1, "GER", 5, 100, false));
        let _e2 = spawn(&mut app, division(2, "GER", 5, 50, false));
        let _e3 = spawn(&mut app, division(3, "GER", 5, 0, false));
        let solo = spawn(&mut app, division(4, "GER", 6, 40, false));
        app.update();
        assert_eq!(bar_width(&mut app, e1, true), 13.0, "орг (100 + 50 + 0) / 3 = 50% от 26 px");
        assert_eq!(bar_width(&mut app, e1, false), 26.0, "прочность у всех 100%");
        assert_eq!(bar_width(&mut app, solo, true), 10.4, "одиночная фишка показывает свою организацию 40%");
    }

    #[test]
    fn standing_divisions_share_the_province_center_without_offset() {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_secs(1)); // сглаживание = 1: сразу в цель
        app.insert_resource(time)
            .insert_resource(ProvinceCenters { centers: [(5, Vec2::new(100.0, 50.0))].into_iter().collect() })
            .add_systems(Update, update_division_positions);
        let ents: Vec<Entity> = (1..=3).map(|id| spawn(&mut app, division(id, "GER", 5, 100, false))).collect();
        app.update();
        for e in ents {
            let t = app.world().get::<Transform>(e).expect("позиция").translation;
            assert_eq!((t.x, t.y), (100.0, 50.0), "никакого диагонального веера");
        }
    }

    #[test]
    fn rmb_orders_the_whole_group_or_only_the_picked_division() {
        let mut app = App::new();
        let e1 = spawn(&mut app, division(1, "GER", 5, 100, false));
        let e2 = spawn(&mut app, division(2, "GER", 5, 100, false));
        let foreign = spawn(&mut app, division(3, "FRA", 5, 100, false));
        let marching = spawn(&mut app, division(4, "GER", 5, 100, true));
        let other = spawn(&mut app, division(5, "GER", 6, 100, false));
        let targets = |app: &mut App, sel: Entity, whole: bool| {
            app.world_mut().run_system_once(move |q: Query<(Entity, &Division)>| order_targets(&q, sel, whole))
        };
        assert_eq!(targets(&mut app, e2, true), vec![e1, e2], "вся группа по id; чужой, марширующий и из другой провинции — нет");
        assert_eq!(targets(&mut app, e2, false), vec![e2], "одна выбранная");
        assert_eq!(targets(&mut app, marching, true), vec![marching]);
        assert_eq!(targets(&mut app, other, true), vec![other]);
        assert_eq!(targets(&mut app, foreign, true), vec![foreign]);
        assert!(targets(&mut app, Entity::PLACEHOLDER, true).is_empty(), "несуществующая сущность — без паники");
    }
}

#[cfg(test)]
mod box_select_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn spawn_at(app: &mut App, id: u32, owner: &str, province: u32, at: Vec2) -> Entity {
        let templates = DivisionTemplates::default();
        let d = Division::from_template(
            id,
            format!("Div {id}"),
            owner.to_string(),
            province,
            BASE_TEMPLATE.to_string(),
            templates.get_or_base(BASE_TEMPLATE),
        );
        app.world_mut().spawn((d, Transform::from_xyz(at.x, at.y, 0.0))).id()
    }

    #[test]
    fn drag_selection_defaults_and_threshold() {
        let mut drag = DragSelection::default();
        assert!(!drag.active);
        assert_eq!(drag.threshold, 8.0);
        drag.start_screen_pos = Vec2::new(100.0, 100.0);
        drag.current_screen_pos = Vec2::new(105.0, 105.0);
        assert!(!drag.is_box(), "7.07 px < 8: это клик");
        drag.current_screen_pos = Vec2::new(108.0, 100.0);
        assert!(!drag.is_box(), "ровно 8 px — ещё не рамка");
        drag.current_screen_pos = Vec2::new(109.0, 100.0);
        assert!(drag.is_box());
    }

    #[test]
    fn rect_is_normalized_from_any_corner_order() {
        let (min, max) = normalized_rect(Vec2::new(10.0, -5.0), Vec2::new(-4.0, 20.0));
        assert_eq!((min, max), (Vec2::new(-4.0, -5.0), Vec2::new(10.0, 20.0)));
        let (min2, max2) = normalized_rect(Vec2::new(-4.0, 20.0), Vec2::new(10.0, -5.0));
        assert_eq!((min, max), (min2, max2));
    }

    #[test]
    fn point_in_aabb_includes_borders_and_rejects_outside() {
        let (min, max) = (Vec2::new(0.0, 0.0), Vec2::new(10.0, 5.0));
        assert!(rect_contains(min, max, Vec2::new(5.0, 2.5)));
        assert!(rect_contains(min, max, min) && rect_contains(min, max, max), "границы включены");
        assert!(!rect_contains(min, max, Vec2::new(-0.1, 2.0)));
        assert!(!rect_contains(min, max, Vec2::new(10.1, 2.0)));
        assert!(!rect_contains(min, max, Vec2::new(5.0, 5.1)));
        assert!(!rect_contains(min, max, Vec2::new(5.0, -1.0)));
        // вырожденная рамка-точка
        assert!(rect_contains(min, min, min));
    }

    #[test]
    fn box_captures_only_player_divisions_inside_sorted_by_id() {
        let mut app = App::new();
        let far = spawn_at(&mut app, 9, "GER", 1, Vec2::new(500.0, 500.0));
        let b = spawn_at(&mut app, 7, "GER", 2, Vec2::new(20.0, 10.0));
        let a = spawn_at(&mut app, 3, "GER", 3, Vec2::new(5.0, 5.0));
        let enemy = spawn_at(&mut app, 4, "FRA", 3, Vec2::new(6.0, 6.0));
        let edge = spawn_at(&mut app, 5, "GER", 4, Vec2::new(30.0, 30.0));
        let result = app.world_mut().run_system_once(|q: Query<(Entity, &Division, &Transform)>| {
            boxed_divisions(
                q.iter().map(|(e, d, t)| (e, d.id, d.owner.as_str(), t.translation.truncate())),
                "GER",
                Vec2::new(0.0, 0.0),
                Vec2::new(30.0, 30.0),
            )
        });
        assert_eq!(result, vec![a, edge, b], "чужие и далёкие не попали; порядок по id 3, 5, 7");
        assert!(!result.contains(&enemy) && !result.contains(&far));
        // пустая рамка
        let none = app.world_mut().run_system_once(|q: Query<(Entity, &Division, &Transform)>| {
            boxed_divisions(
                q.iter().map(|(e, d, t)| (e, d.id, d.owner.as_str(), t.translation.truncate())),
                "GER",
                Vec2::new(-100.0, -100.0),
                Vec2::new(-50.0, -50.0),
            )
        });
        assert!(none.is_empty());
    }

    #[test]
    fn rmb_orders_every_division_of_the_box_group() {
        let mut app = App::new();
        let a = spawn_at(&mut app, 1, "GER", 1, Vec2::ZERO);
        let b = spawn_at(&mut app, 2, "GER", 2, Vec2::ZERO);
        let c = spawn_at(&mut app, 3, "GER", 3, Vec2::ZERO);
        let dead = spawn_at(&mut app, 4, "GER", 4, Vec2::ZERO);
        app.world_mut().despawn(dead);
        let multi = SelectedDivisions(vec![a, b, dead, c]);
        let targets = {
            let multi = multi.clone();
            app.world_mut().run_system_once(move |q: Query<(Entity, &Division)>| command_targets(&multi, &q, a, false))
        };
        assert_eq!(targets, vec![a, b, c], "группа рамки целиком, погибшие отфильтрованы, независимо от режима стека");
        // без рамки действует прежняя логика: одна выбранная дивизия
        let empty = SelectedDivisions::default();
        let single = app.world_mut().run_system_once(move |q: Query<(Entity, &Division)>| command_targets(&empty, &q, b, false));
        assert_eq!(single, vec![b]);
    }
}

#[cfg(test)]
mod nato_tests {
    use super::*;
    use crate::units::BattalionDef;
    use bevy::ecs::system::RunSystemOnce;

    fn symbol_of(name: &str) -> NatoSymbol {
        DivisionTemplates::default().get_or_base(name).nato_symbol()
    }

    #[test]
    fn real_templates_get_their_symbols() {
        assert_eq!(symbol_of("Infanterie-Division"), NatoSymbol::Infantry, "9 пехотных; артиллерия поддержки не в счёт");
        assert_eq!(symbol_of("Panzer-Division"), NatoSymbol::Armored, "есть лёгкие танки");
        assert_eq!(symbol_of("Infanterie-Division (mot.)"), NatoSymbol::Motorized);
        // неизвестное имя -> базовая пехота
        assert_eq!(symbol_of("Gebirgs-Brigade"), NatoSymbol::Infantry);
    }

    #[test]
    fn stats_and_division_carry_the_template_symbol() {
        let templates = DivisionTemplates::default();
        let panzer = templates.get_or_base("Panzer-Division");
        assert_eq!(panzer.calculate_stats().nato_symbol, NatoSymbol::Armored);
        let mut d = Division::from_template(1, "d".into(), "GER".into(), 1, "Panzer-Division".into(), panzer);
        assert_eq!(d.nato_symbol, NatoSymbol::Armored);
        // переформирование меняет символ вместе с шаблоном
        d.refit(templates.get_or_base("Infanterie-Division"));
        assert_eq!(d.nato_symbol, NatoSymbol::Infantry);
    }

    fn template_of(battalions: Vec<BattalionDef>) -> DivisionTemplate {
        DivisionTemplate { name: "t".into(), battalions }
    }

    #[test]
    fn symbol_rules_cover_cavalry_artillery_and_mixes() {
        let horse = BattalionDef { speed_kph: 80, ..BattalionDef::infantry() };
        assert_eq!(horse.nato_symbol(), NatoSymbol::Cavalry);
        assert_eq!(template_of(vec![horse.clone(); 4]).nato_symbol(), NatoSymbol::Cavalry);
        let gun = BattalionDef { equipment_needs: (EquipmentType::Artillery, 24), ..BattalionDef::infantry() };
        assert_eq!(template_of(vec![gun.clone(), gun.clone(), BattalionDef::infantry()]).nato_symbol(), NatoSymbol::Artillery);
        // пехота с одним орудийным батальоном — всё ещё пехота
        assert_eq!(template_of(vec![gun, BattalionDef::infantry(), BattalionDef::infantry()]).nato_symbol(), NatoSymbol::Infantry);
        // один танковый батальон делает дивизию бронетанковой
        assert_eq!(
            template_of(vec![BattalionDef::light_tank(), BattalionDef::infantry(), BattalionDef::infantry()]).nato_symbol(),
            NatoSymbol::Armored
        );
        // половина моторизованных — мотопехота, меньше — пехота
        let mot = BattalionDef::motorized();
        assert_eq!(template_of(vec![mot.clone(), BattalionDef::infantry()]).nato_symbol(), NatoSymbol::Motorized);
        assert_eq!(template_of(vec![mot, BattalionDef::infantry(), BattalionDef::infantry()]).nato_symbol(), NatoSymbol::Infantry);
        // пустой шаблон и одни роты поддержки не паникуют
        assert_eq!(template_of(vec![]).nato_symbol(), NatoSymbol::Infantry);
        assert_eq!(template_of(vec![BattalionDef::engineers()]).nato_symbol(), NatoSymbol::Infantry);
    }

    #[test]
    fn strokes_match_each_symbol_and_stay_inside_the_chip() {
        assert_eq!(symbol_strokes(NatoSymbol::Infantry).len(), 2, "крест X");
        assert_eq!(symbol_strokes(NatoSymbol::Cavalry).len(), 1, "одна черта");
        assert_eq!(symbol_strokes(NatoSymbol::Motorized).len(), 4, "X и два колеса");
        assert_eq!(symbol_strokes(NatoSymbol::Armored).len(), 12, "эллипс из 12 звеньев");
        let dot = symbol_strokes(NatoSymbol::Artillery);
        assert_eq!(dot.len(), 1);
        assert_eq!((dot[0].0, dot[0].1), (dot[0].2, dot[0].3), "точка");
        // всё внутри внутренней рамки фишки и над подложкой полосок (y > -3 px)
        let half_w = DIVISION_SIZE.x as i32 - 2;
        for symbol in [NatoSymbol::Infantry, NatoSymbol::Cavalry, NatoSymbol::Motorized, NatoSymbol::Armored, NatoSymbol::Artillery] {
            for (x1, y1, x2, y2) in symbol_strokes(symbol) {
                for (x, y) in [(x1, y1), (x2, y2)] {
                    assert!(x.abs() <= half_w, "{symbol:?}: x = {x} hp");
                    assert!(y > -6 && y <= DIVISION_SIZE.y as i32 - 3, "{symbol:?}: y = {y} hp");
                }
            }
        }
        // ломаная эллипса замкнута
        let ring = symbol_strokes(NatoSymbol::Armored);
        for k in 0..12 {
            assert_eq!((ring[k].2, ring[k].3), (ring[(k + 1) % 12].0, ring[(k + 1) % 12].1));
        }
    }

    #[test]
    fn line_color_contrasts_with_the_nation_color() {
        assert_eq!(symbol_line_color([240, 240, 220]), Color::srgb(0.02, 0.02, 0.02), "светлая нация — чёрные линии");
        assert_eq!(symbol_line_color([20, 40, 90]), Color::srgb(0.98, 0.98, 0.98), "тёмная — белые");
    }

    #[test]
    fn inspector_badge_text() {
        assert_eq!(nato_badge_text(NatoSymbol::Infantry), "[NATO: Пехота]");
        assert_eq!(nato_badge_text(NatoSymbol::Armored), "[NATO: Танки]");
        assert_eq!(nato_badge_text(NatoSymbol::Motorized), "[NATO: Мотопехота]");
    }

    fn strokes_of(app: &mut App, root: Entity) -> usize {
        let mut q = app.world_mut().query_filtered::<&Parent, With<NatoStroke>>();
        q.iter(app.world()).filter(|p| p.get() == root).count()
    }

    #[test]
    fn chip_is_drawn_with_its_symbol_and_redrawn_after_refit() {
        let mut app = App::new();
        app.insert_resource(CountryColors::default());
        let templates = DivisionTemplates::default();
        let division = Division::from_template(1, "d".into(), "GER".into(), 1, "Panzer-Division".into(), templates.get_or_base("Panzer-Division"));
        app.world_mut().run_system_once(move |mut commands: Commands, colors: Res<CountryColors>| {
            spawn_division_entity(&mut commands, division.clone(), Vec2::ZERO, &colors);
        });
        let root = app.world_mut().query_filtered::<Entity, With<Division>>().single(app.world());
        assert_eq!(strokes_of(&mut app, root), 12, "танковый эллипс");
        // переформирование в пехотную дивизию: значок меняется на крест X
        let infantry = templates.get_or_base("Infanterie-Division").clone();
        app.world_mut().get_mut::<Division>(root).unwrap().refit(&infantry);
        app.world_mut().run_system_once(update_nato_icons);
        assert_eq!(strokes_of(&mut app, root), 2, "старые штрихи удалены, нарисован крест");
        assert_eq!(app.world().get::<NatoIcon>(root).unwrap().0, NatoSymbol::Infantry);
        // без изменений ничего не пересоздаётся
        app.world_mut().run_system_once(update_nato_icons);
        assert_eq!(strokes_of(&mut app, root), 2);
    }
}

#[cfg(test)]
mod sovereignty_tests {
    use super::*;

    /// Кёнигсберг (1, GER) – Польша (2, POL) – Берлин (3, GER) цепочкой; 4 (FRA) за Берлином.
    struct Scene {
        adjacency: ProvinceAdjacency,
        centers: ProvinceCenters,
        map: ProvinceMap,
        controllers: ProvinceControllers,
        metrics: GraphMetrics,
    }

    fn world(owners: &[(u32, &str)]) -> Scene {
        let neighbors: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::from([
            (1, BTreeSet::from([2])),
            (2, BTreeSet::from([1, 3])),
            (3, BTreeSet::from([2, 4])),
            (4, BTreeSet::from([3])),
        ]);
        let adjacency = ProvinceAdjacency { neighbors };
        let centers = ProvinceCenters { centers: (1..=4).map(|p| (p, Vec2::new(p as f32 * 100.0, 0.0))).collect() };
        let metrics = GraphMetrics::compute(&adjacency, &centers);
        Scene {
            adjacency,
            centers,
            map: ProvinceMap::empty_for_test(),
            controllers: ProvinceControllers(owners.iter().map(|&(p, c)| (p, c.to_string())).collect()),
            metrics,
        }
    }

    fn path(w: &Scene, from: u32, to: u32, owner: &str, dip: &DiplomacyRegistry) -> Option<Vec<u32>> {
        find_path_a_star(from, to, &w.adjacency, &w.centers, &w.map, &w.controllers, &w.metrics, owner, dip, 40)
    }

    #[test]
    fn neutral_poland_blocks_the_path_from_koenigsberg_in_peacetime() {
        let w = world(&[(1, "GER"), (2, "POL"), (3, "GER"), (4, "FRA")]);
        let peace = DiplomacyRegistry::default();
        assert_eq!(path(&w, 1, 3, "GER", &peace), None, "анклав: сухопутного пути по своим землям нет");
        assert_eq!(path(&w, 1, 2, "GER", &peace), None, "и сама Польша закрыта");
        assert_eq!(path(&w, 3, 1, "GER", &peace), None, "обратно тоже");
    }

    #[test]
    fn path_through_an_enemy_country_exists_only_at_war() {
        let w = world(&[(1, "GER"), (2, "POL"), (3, "GER"), (4, "FRA")]);
        let mut dip = DiplomacyRegistry::default();
        dip.declare_war("GER", "POL");
        assert_eq!(path(&w, 1, 3, "GER", &dip), Some(vec![1, 2, 3]), "война с Польшей открывает проход");
        assert_eq!(path(&w, 3, 4, "GER", &dip), None, "Франция по-прежнему нейтральна");
        dip.declare_war("GER", "FRA");
        assert_eq!(path(&w, 1, 4, "GER", &dip), Some(vec![1, 2, 3, 4]));
    }

    #[test]
    fn military_access_opens_a_neutral_country_for_the_guest_only() {
        let w = world(&[(1, "GER"), (2, "POL"), (3, "GER"), (4, "FRA")]);
        let mut dip = DiplomacyRegistry::default();
        dip.grant_access("POL", "GER");
        assert_eq!(path(&w, 1, 3, "GER", &dip), Some(vec![1, 2, 3]));
        assert_eq!(path(&w, 3, 1, "FRA", &dip), None, "право есть у GER, но не у FRA");
    }

    #[test]
    fn own_lands_and_degenerate_goals_do_not_panic() {
        let w = world(&[(1, "GER"), (2, "GER"), (3, "GER")]);
        let peace = DiplomacyRegistry::default();
        assert_eq!(path(&w, 1, 3, "GER", &peace), Some(vec![1, 2, 3]));
        assert_eq!(path(&w, 1, 1, "GER", &peace), Some(vec![1]));
        assert_eq!(path(&w, 1, 99, "GER", &peace), None, "неизвестная провинция");
        assert_eq!(path(&w, 1, 4, "GER", &peace), None, "у провинции 4 нет контроллера");
    }

    #[test]
    fn rmb_on_a_neutral_province_issues_no_order() {
        let mut ecs = World::new();
        let (a, b) = (ecs.spawn_empty().id(), ecs.spawn_empty().id());
        let mut dip = DiplomacyRegistry::default();
        assert!(rmb_orders(&dip, "GER", Some("POL"), 7, &[a, b]).is_empty(), "нейтрал игнорируется");
        assert!(rmb_orders(&dip, "GER", None, 7, &[a, b]).is_empty(), "море / неизвестная провинция");
        let own = rmb_orders(&dip, "GER", Some("GER"), 7, &[a, b]);
        assert_eq!(own.iter().map(|o| (o.division, o.target)).collect::<Vec<_>>(), vec![(a, 7), (b, 7)]);
        dip.declare_war("GER", "POL");
        assert_eq!(rmb_orders(&dip, "GER", Some("POL"), 7, &[a]).len(), 1, "на войне вход разрешён");
    }
}

#[cfg(test)]
mod experience_tests {
    use super::*;
    use crate::units::DivisionExperienceLevel as Level;

    fn make(id: u32, xp: u32) -> Division {
        let templates = DivisionTemplates::default();
        let mut d = Division::from_template(
            id,
            "d".into(),
            "GER".into(),
            1,
            PLAIN_INFANTRY_TEMPLATE.to_string(),
            templates.get_or_base(PLAIN_INFANTRY_TEMPLATE),
        );
        d.experience_pct = xp;
        d
    }

    #[test]
    fn rank_boundaries_follow_the_canon() {
        let cases = [
            (0, Level::Green, 75),
            (9, Level::Green, 75),
            (10, Level::Trained, 100),
            (29, Level::Trained, 100),
            (30, Level::Regular, 125),
            (59, Level::Regular, 125),
            (60, Level::Seasoned, 150),
            (89, Level::Seasoned, 150),
            (90, Level::Veteran, 175),
            (100, Level::Veteran, 175),
            (250, Level::Veteran, 175),
        ];
        for (pct, level, multiplier) in cases {
            let got = Level::from_experience_pct(pct);
            assert_eq!(got, level, "{pct}%");
            assert_eq!(got.combat_multiplier_pct(), multiplier, "{pct}%");
        }
        let titles: Vec<&str> =
            [Level::Green, Level::Trained, Level::Regular, Level::Seasoned, Level::Veteran].iter().map(Level::title).collect();
        assert_eq!(titles, vec!["Новобранцы", "Обученные", "Кадровые", "Опытные", "Ветераны"]);
    }

    #[test]
    fn new_divisions_are_green_and_oob_experience_sets_the_rank() {
        let d = make(1, 0);
        assert_eq!(d.experience_level(), Level::Green);
        assert_eq!(Division::from_template(1, "d".into(), "GER".into(), 1, BASE_TEMPLATE.into(), DivisionTemplates::default().get_or_base(BASE_TEMPLATE)).experience_pct, 0);
        // стартовые 30% из GER_1936 -> Кадровые, +25%
        assert_eq!(make(1, 30).experience_multiplier_pct(), 125);
    }

    #[test]
    fn ui_text_shows_rank_and_bonus() {
        assert_eq!(experience_text(30), "Опыт: 30% (Кадровые, +25% к бою)");
        assert_eq!(experience_text(0), "Опыт: 0% (Новобранцы, -25% к бою)");
        assert_eq!(experience_text(15), "Опыт: 15% (Обученные, 0% к бою)");
        assert_eq!(experience_text(95), "Опыт: 95% (Ветераны, +75% к бою)");
    }

    #[test]
    fn combat_hours_grow_experience_up_to_the_cap() {
        let mut d = make(1, 10);
        for _ in 0..XP_HOURS_PER_PCT - 1 {
            d.gain_combat_experience_hour();
        }
        assert_eq!(d.experience_pct, 10, "накопление ещё не дало процента");
        d.gain_combat_experience_hour();
        assert_eq!((d.experience_pct, d.combat_xp_hours), (11, 0));
        for _ in 0..24 {
            d.gain_combat_experience_hour();
        }
        assert_eq!(d.experience_pct, 15, "сутки на линии = +4%");
        let mut veteran = make(2, 99);
        for _ in 0..100 {
            veteran.gain_combat_experience_hour();
        }
        assert_eq!(veteran.experience_pct, 100, "потолок 100%");
    }

    #[test]
    fn reinforcements_dilute_experience_as_a_weighted_average() {
        // ветераны 90%, потеряли половину состава и получили её обратно: 500 * 90 / 1000 = 45% (Кадровые)
        let mut d = make(1, 90);
        d.max_strength = 1000;
        d.current_strength = 500;
        d.dilute_experience(500);
        assert_eq!(d.experience_pct, 45);
        assert_eq!(d.experience_level(), Level::Regular);
        // без пополнения и при нулевой прочности ничего не ломается
        d.dilute_experience(0);
        assert_eq!(d.experience_pct, 45);
        let mut empty = make(2, 50);
        empty.current_strength = 0;
        empty.dilute_experience(100);
        assert_eq!(empty.experience_pct, 0, "полностью новые люди — 0%");
        let mut nothing = make(3, 50);
        nothing.current_strength = 0;
        nothing.dilute_experience(0);
        assert_eq!(nothing.experience_pct, 50, "ни прочности, ни пополнения: без деления на ноль");
    }

    #[test]
    fn daily_reinforcement_dilutes_experience_of_a_veteran_division() {
        use crate::economy::{CountryEconomy, Economies};
        let mut app = App::new();
        let mut eco = CountryEconomy::new_abundant(10, 10);
        eco.set_stock(EquipmentType::InfantryEquipment, 100_000);
        app.add_event::<DailyTickEvent>()
            .insert_resource(Economies(BTreeMap::from([("GER".to_string(), eco)])))
            .add_systems(Update, crate::economy::daily_reinforcement);
        let mut d = make(1, 90);
        d.current_strength = d.max_strength / 2;
        let half = d.current_strength;
        let e = app.world_mut().spawn(d).id();
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
        let d = app.world().get::<Division>(e).expect("дивизия");
        let gained = d.current_strength - half;
        assert!(gained > 0, "пополнение пришло");
        assert_eq!(d.experience_pct, (u64::from(half) * 90 / u64::from(half + gained)) as u32, "формула размытия");
        assert!(d.experience_pct < 90);
    }

    #[test]
    fn experience_scales_attack_and_defense_in_combat() {
        let green = make(1, 0);
        let trained = make(2, 10);
        let regular = make(3, 30);
        let veteran = make(4, 95);
        // атака 90, защита 225 у всех. Ровня (Trained vs Trained): 800 * 90 / 315 = 228
        assert_eq!(combat_bp(&trained, &make(5, 10), 0), 228);
        // атака ветерана 90 * 175 / 100 = 157 против защиты 225: 800 * 157 / 382 = 328
        assert_eq!(combat_bp(&veteran, &trained, 0), 328);
        // атака новобранца 90 * 75 / 100 = 67: 800 * 67 / 292 = 183
        assert_eq!(combat_bp(&green, &trained, 0), 183);
        // защита ветерана 225 * 175 / 100 = 393: 800 * 90 / 483 = 149
        assert_eq!(combat_bp(&trained, &veteran, 0), 149);
        // защита новобранца 225 * 75 / 100 = 168: 800 * 90 / 258 = 279
        assert_eq!(combat_bp(&trained, &green, 0), 279);
        // атака кадровых 90 * 125 / 100 = 112: 800 * 112 / 337 = 265
        assert_eq!(combat_bp(&regular, &trained, 0), 265);
        // чем опытнее атакующий, тем сильнее удар
        assert!(combat_bp(&veteran, &trained, 0) > combat_bp(&regular, &trained, 0));
        assert!(combat_bp(&regular, &trained, 0) > combat_bp(&trained, &trained, 0));
    }
}
