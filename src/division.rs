//! Дивизии: спавн, выбор (ЛКМ), приказы на марш по графу провинций (ПКМ),
//! движение и бой по часовым тикам `GameClock`.
//!
//! Ввод только формирует событие `OrderMove`; логика (`movement`, `current_province`, бой)
//! меняется в `apply_orders`, `advance_marches` и `resolve_combats` целыми числами по `HourTick`.
//! Позиции, масштаб спрайтов и UI-полоски — чисто визуальные и на симуляцию не влияют.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashSet};

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts};

use crate::camera::MainCamera;
use crate::clock::{ClockSet, DailyTickEvent, HourTick};
use crate::air::{air_attack_bonus_pct, air_defense_penalty_pct, air_status, side_edge, AirSuperiorityMap};
use crate::command::SKILL_BONUS_PCT;
use crate::economy::{update_supply_network, SupplyNetwork};
use crate::oob::OobDivisions;
use crate::units::{DivisionTemplate, DivisionTemplates, BASE_TEMPLATE, INFANTRY_SPEED_KPH};
#[cfg(test)]
use crate::units::PLAIN_INFANTRY_TEMPLATE;
use crate::map::{
    country_color, pick_province, setup_map, HoveredProvince, ProvinceAdjacency, ProvinceCenters, ProvinceControllerChanged,
    ProvinceControllers, ProvinceMap, ProvinceRivers, StateRegistry,
};

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

const DIVISION_SIZE: Vec2 = Vec2::new(24.0, 14.0);
/// Сдвиг каждой следующей дивизии в стеке, экранные пиксели (по диагонали вправо-вниз).
const STACK_STEP_PX: Vec2 = Vec2::new(10.0, -8.0);
const FRAME_COLOR: Color = Color::srgb(0.9, 0.9, 0.86);
const SELECTED_FRAME_COLOR: Color = Color::srgb(1.0, 0.85, 0.1);
const FILL_COLOR: Color = Color::srgb(0.14, 0.16, 0.2);
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
    pub required_equipment: u32,
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

    /// Снаряжение сейчас: пропорционально укомплектованности.
    pub fn current_equipment(&self) -> u32 {
        (u64::from(self.required_equipment) * u64::from(self.current_strength) / u64::from(self.max_strength.max(1)))
            as u32
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
            required_equipment: stats.total_required_equipment,
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
}

impl ActiveCombat {
    /// Участвует ли дивизия в бою (на линии или в резерве).
    pub fn involves(&self, entity: Entity) -> bool {
        self.attackers.contains(&entity) || self.defenders.contains(&entity)
    }
}

#[derive(Resource, Default)]
pub struct SelectedDivision(pub Option<Entity>);

/// Приказ на перемещение дивизии в соседнюю провинцию.
#[derive(Event, Debug, Clone, Copy)]
pub struct OrderMove {
    pub division: Entity,
    pub target: u32,
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
            .init_resource::<DivisionTemplates>()
            .add_event::<OrderMove>()
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
                )
                    .chain()
                    .after(pick_province)
                    .after(ClockSet)
                    .in_set(CombatSet),
            )
            .add_systems(Update, (combat_ui, division_info_ui))
            .add_systems(Update, daily_entrenchment.after(ClockSet))
            .add_systems(Update, daily_supply.after(update_supply_network));
    }
}

// ---------------------------------------------------------------------------
// Спавн
// ---------------------------------------------------------------------------

fn spawn_division_entity(commands: &mut Commands, division: Division, center: Vec2) {
    let [r, g, b] = country_color(&division.owner);

    // Корень — рамка; внутри тёмная заливка и цветной бейдж страны.
    commands
        .spawn((
            SpriteBundle {
                sprite: Sprite { color: FRAME_COLOR, custom_size: Some(DIVISION_SIZE), ..default() },
                transform: Transform::from_xyz(center.x, center.y, DIVISION_Z),
                ..default()
            },
            division,
        ))
        .with_children(|p| {
            p.spawn(SpriteBundle {
                sprite: Sprite {
                    color: FILL_COLOR,
                    custom_size: Some(DIVISION_SIZE - Vec2::splat(2.0)),
                    ..default()
                },
                transform: Transform::from_xyz(0.0, 0.0, 0.1),
                ..default()
            });
            p.spawn(SpriteBundle {
                sprite: Sprite {
                    color: Color::srgb_u8(r, g, b),
                    custom_size: Some(Vec2::new(8.0, 8.0)),
                    ..default()
                },
                transform: Transform::from_xyz(-6.0, 0.0, 0.2),
                ..default()
            });
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
    templates: Res<DivisionTemplates>,
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
        base_stats.total_required_equipment
    );

    let mut next_id = 1u32;
    let mut skipped = 0u32;
    // Счётчики по странам, порядок вывода детерминирован
    let mut per_owner: BTreeMap<&str, u32> = BTreeMap::new();

    for entry in &oob.0 {
        let Some(center) = centers.centers.get(&entry.location) else {
            skipped += 1;
            continue;
        };
        if is_water(&map, entry.location) {
            skipped += 1;
            continue;
        }
        // Шаблон из реестра; незарегистрированные (французские и т.п.) пока считаются базовым.
        let division = Division::from_template(
            next_id,
            entry.name.clone(),
            entry.owner.clone(),
            entry.location,
            entry.template.clone(),
            templates.get_or_base(&entry.template),
        );
        spawn_division_entity(&mut commands, division, *center);
        *per_owner.entry(entry.owner.as_str()).or_default() += 1;
        next_id += 1;
    }

    let breakdown: Vec<String> = per_owner.iter().map(|(tag, n)| format!("{tag} {n}")).collect();
    println!(
        "[OOB] Заспавнено дивизий: {} ({}), пропущено (море / нет центра): {skipped}",
        next_id - 1,
        breakdown.join(", ")
    );
}

// ---------------------------------------------------------------------------
// Ввод (формирует только выбор / события)
// ---------------------------------------------------------------------------

/// ЛКМ: выбрать дивизию в провинции под курсором; клик в другое место снимает выбор.
fn select_division_input(
    mouse: Res<ButtonInput<MouseButton>>,
    hovered: Res<HoveredProvince>,
    divisions: Query<(Entity, &Division)>,
    mut selected: ResMut<SelectedDivision>,
    mut contexts: EguiContexts,
) {
    if !mouse.just_pressed(MouseButton::Left) || contexts.try_ctx_mut().is_none_or(|ctx| ctx.wants_pointer_input()) {
        return;
    }
    selected.0 = hovered.id.and_then(|province| {
        // Стек провинции: стоящие дивизии по возрастанию id — детерминированный порядок
        let mut stack: Vec<(u32, Entity)> = divisions
            .iter()
            .filter(|(_, d)| d.current_province == province && d.movement.is_none())
            .map(|(e, d)| (d.id, e))
            .collect();
        stack.sort_unstable();

        if stack.is_empty() {
            // Остались только идущие в марше (их фишки вне центра): выбираем с наименьшим id
            return divisions
                .iter()
                .filter(|(_, d)| d.current_province == province)
                .min_by_key(|(_, d)| d.id)
                .map(|(e, _)| e);
        }
        // Циклический выбор: если выбрана дивизия этой провинции — следующая, после последней — первая
        let next = match selected.0.and_then(|cur| stack.iter().position(|&(_, e)| e == cur)) {
            Some(i) => (i + 1) % stack.len(),
            None => 0,
        };
        Some(stack[next].1)
    });
}

/// ПКМ (короткий клик без перетаскивания, т.к. ПКМ-drag — панорама камеры) -> `OrderMove`.
fn order_move_input(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    hovered: Res<HoveredProvince>,
    selected: Res<SelectedDivision>,
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
    if let (Some(division), Some(target)) = (selected.0, hovered.id) {
        orders.send(OrderMove { division, target });
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
/// Суверенитет: провинция проходима, только если её контроллер — `owner` или `enemy_target`.
/// Нейтральные страны и провинции без контроллера — стена. Стартовая провинция не проверяется
/// (дивизия может стоять где угодно), а цель обязана быть проходимой.
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
    enemy_target: Option<&str>,
    speed: u32,
) -> Option<Vec<u32>> {
    let is_passable = |province: u32| {
        controllers
            .0
            .get(&province)
            .is_some_and(|c| c.as_str() == owner || enemy_target == Some(c.as_str()))
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
) {
    for order in events.read() {
        if combats.iter().any(|c| c.involves(order.division)) {
            println!("[ПРИКАЗ] Дивизия в бою — приказ отклонён");
            continue;
        }
        let Ok(mut division) = divisions.get_mut(order.division) else { continue };

        // Если дивизия уже в пути, новый маршрут строится от точки, куда она сейчас идёт.
        let start = division.movement.map_or(division.current_province, |m| m.to);
        // Цель приказа определяет «противника»: путь идёт по своей территории и по территории
        // страны, контролирующей целевую провинцию; чужие нейтральные страны — стена.
        let enemy_target = controllers.0.get(&order.target).map(String::as_str);
        match find_path_a_star(
            start,
            order.target,
            &adjacency,
            &centers,
            &map,
            &controllers,
            &metrics,
            &division.owner,
            enemy_target,
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
            commands.spawn(ActiveCombat {
                province_id: province,
                attacker_owner: owner,
                defender_owner,
                attackers,
                defenders,
                max_width: BASE_COMBAT_WIDTH,
                combat_progress: 0,
                elapsed_hours: 0,
            });
        }
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
        Some("urban") => (30, 20, 0),
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
}

impl HitContext {
    /// Без модификаторов (удары обороняющихся и тесты без рельефа).
    pub const NONE: HitContext =
        HitContext { attack_penalty: 0, defense_bonus: 0, air_attack_bonus: 0, air_defense_penalty: 0 };

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

    let base_defense = def.defense * def.current_strength / def.max_strength.max(1);
    // Окопанность: +2% защиты за пункт (у наступающих она всегда 0 — марш её сбрасывает)
    let mut defense = base_defense * (100 + def.entrenchment.min(def.max_entrenchment) * 2) / 100;
    defense = defense * (100 + def.command_defense_skill * SKILL_BONUS_PCT) / 100;
    if def.over_command_limit {
        defense = defense * 50 / 100;
    }
    if !def.is_supplied {
        defense = defense * UNSUPPLIED_STAT_PCT / 100; // отрезан от снабжения: -33%
    }
    defense = defense * (100 + ctx.defense_bonus) / 100; // бонус рельефа обороняющимся
    defense = defense * (100 - ctx.air_defense_penalty.min(100)) / 100; // вражеская авиация: до -15%
    (COMBAT_INTENSITY_BP * attack / (attack + defense).max(1)).max(1)
}

/// Правило брони для удара `striker` по `target` (целые числа):
/// - цель бронирована сильнее, чем пробитие бьющего (`armor(target) > piercing(striker)`) —
///   урон вдвое меньше: пехота не пробивает броню;
/// - бьющий бронирован сильнее, чем пробитие цели (`armor(striker) > piercing(target)`) —
///   урон на 50% больше: безнаказанный расстрел с позиций.
/// Для пары «A атакует B» это ровно условие `armor(B) > piercing(A)`: удар A по B режется вдвое,
/// ответный удар B по A усиливается; правило симметрично и работает, если бронирован атакующий.
fn apply_armor_rule(damage: u32, striker: &Division, target: &Division) -> u32 {
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
    registry: Res<StateRegistry>,
    air: Res<AirSuperiorityMap>,
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
                    let fits = used == 0 || used + width <= state.max_width;
                    if fits {
                        used += width;
                    }
                    set_combat_state(&mut divisions, e, if fits { CombatState::Active } else { CombatState::Reserve });
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
            let air_value = registry.state_of(province).map_or(0, |s| air.get(s.id));
            let (att_air, def_air) =
                (side_edge(air_value, &state.attacker_owner), side_edge(air_value, &state.defender_owner));
            let advantage = |damage: u32, has_advantage: bool| {
                if has_advantage { damage * (100 + TACTICAL_ADVANTAGE_PCT) / 100 } else { damage }
            };

            if !att_line.is_empty() && !def_line.is_empty() {
                // Все удары считаются по значениям до боя, затем применяются
                let mut hits: Vec<(Entity, u32, u32)> = Vec::new();
                let terrain = terrain_modifiers(map.terrain_of(province));
                for (k, (_, striker)) in att_line.iter().enumerate() {
                    let (target_e, target) = &def_line[k % def_line.len()];
                    // Река между провинцией отправления этой дивизии и обороняемой — форсирование
                    let over_river = striker.movement.is_some_and(|m| rivers.crosses(m.from, province));
                    let ctx = HitContext::for_attack(&terrain, striker, over_river)
                        .with_air(air_attack_bonus_pct(att_air), air_defense_penalty_pct(def_air));
                    // Наступающая дивизия получает бонус планирования к атаке
                    let bp = combat_bp_ctx(striker, target, striker.planning, ctx);
                    // Броня меняет урон и по организации, и по прочности; разведка добавляет +5%
                    let org_milli = apply_armor_rule(target.max_org * bp / 10, striker, target).max(1);
                    let strength = apply_armor_rule(target.max_strength * bp / 50_000, striker, target).max(1);
                    let tactical = recon_adv == Some(true);
                    hits.push((*target_e, advantage(org_milli, tactical), advantage(strength, tactical)));
                }
                for (k, (_, striker)) in def_line.iter().enumerate() {
                    let (target_e, target) = &att_line[k % att_line.len()];
                    // обороняющиеся бонуса планирования и рельефа не имеют, небо действует на обе стороны
                    let ctx = HitContext::NONE.with_air(air_attack_bonus_pct(def_air), air_defense_penalty_pct(att_air));
                    let bp = combat_bp_ctx(striker, target, 0, ctx);
                    // Броня меняет урон и по организации, и по прочности; разведка добавляет +5%
                    let org_milli = apply_armor_rule(target.max_org * bp / 10, striker, target).max(1);
                    let strength = apply_armor_rule(target.max_strength * bp / 50_000, striker, target).max(1);
                    let tactical = recon_adv == Some(false);
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
                    if st != CombatState::Reserve || used + width > state.max_width {
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
    cameras: Query<&OrthographicProjection, With<MainCamera>>,
    mut stationary: Local<Vec<(u32, u32)>>,
    mut query: Query<(&Division, &mut Transform)>,
) {
    let smooth = (VISUAL_SMOOTHING * time.delta_seconds()).min(1.0);
    let zoom = cameras.get_single().map_or(1.0, |p| p.scale);

    // Стоящие дивизии (провинция, id) в отсортированном виде: слот в стеке = номер среди дивизий
    // той же провинции. Сдвиг только визуальный, на симуляцию не влияет.
    stationary.clear();
    stationary.extend(query.iter().filter(|(d, _)| d.movement.is_none()).map(|(d, _)| (d.current_province, d.id)));
    stationary.sort_unstable();

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
            None => match centers.centers.get(&division.current_province) {
                Some(c) => {
                    let key = (division.current_province, division.id);
                    let first = stationary.partition_point(|&(p, _)| p < division.current_province);
                    let slot = stationary.binary_search(&key).map_or(0, |pos| pos - first);
                    // Шаг сдвига задан в экранных пикселях, поэтому масштабируется вместе с зумом
                    *c + STACK_STEP_PX * (slot as f32) * zoom
                }
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
    /// Сумма разведки и число дивизий на линии — для средней разведки стороны.
    recon_sum: u32,
    active: u32,
}

fn side_stats(divisions: &Query<&Division>, list: &[Entity]) -> SideStats {
    let mut stats = SideStats { used_width: 0, reserves: 0, org: 0, org_max: 0, recon_sum: 0, active: 0 };
    for d in list.iter().filter_map(|&e| divisions.get(e).ok()) {
        match d.combat_state {
            CombatState::Active => {
                stats.used_width += d.combat_width;
                stats.org += d.current_org;
                stats.org_max += d.max_org;
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

/// Строка атаки в панели дивизии: `Атака: 90 мягк. / 0 твёрд. | Твёрдость: 0%`.
fn attack_label(d: &Division) -> String {
    format!("Атака: {} мягк. / {} твёрд. | Твёрдость: {}%", d.soft_attack, d.hard_attack, d.hardness)
}

/// Строка разведки и неба в окне боя: `Разведка: 3.0 vs 1.0 | Небо: Германия +40`.
fn recon_air_label(att: &SideStats, def: &SideStats, air_value: i32) -> String {
    format!(
        "Разведка: {} vs {} | Небо: {}",
        recon_text(att.recon_sum, att.active),
        recon_text(def.recon_sum, def.active),
        air_status(air_value)
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

/// Маркер боя над провинцией: круглый бейдж с процентом, организация линии, ширина фронта и резерв.
fn combat_ui(
    mut contexts: EguiContexts,
    combats: Query<(Entity, &ActiveCombat)>,
    divisions: Query<&Division>,
    centers: Res<ProvinceCenters>,
    map: Res<ProvinceMap>,
    rivers: Res<ProvinceRivers>,
    registry: Res<StateRegistry>,
    air: Res<AirSuperiorityMap>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    let Ok((camera, cam_tf)) = cameras.get_single() else { return };
    let Some(ctx) = contexts.try_ctx_mut() else { return };

    for (ce, combat) in &combats {
        let Some(center) = centers.centers.get(&combat.province_id) else { continue };
        let Some(screen) = camera.world_to_viewport(cam_tf, center.extend(0.0)) else { continue };
        let att = side_stats(&divisions, &combat.attackers);
        let def = side_stats(&divisions, &combat.defenders);
        // Рельеф обороняемой провинции и форсирование реки хотя бы одной атакующей дивизией
        // (в плашке — самый тяжёлый штраф среди форсирующих: сапёры его снижают)
        let river_penalty = combat
            .attackers
            .iter()
            .filter_map(|&e| divisions.get(e).ok())
            .filter(|d| d.movement.is_some_and(|m| rivers.crosses(m.from, combat.province_id)))
            .map(river_penalty_for)
            .max();
        let terrain_text = terrain_label(map.terrain_of(combat.province_id), river_penalty);
        let air_value = registry.state_of(combat.province_id).map_or(0, |s| air.get(s.id));
        let recon_text = recon_air_label(&att, &def, air_value);

        egui::Area::new(egui::Id::new(("combat_plate", ce)))
            .fixed_pos(egui::pos2(screen.x - 28.0, screen.y - 70.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                // Бейдж: красный круг с процентом победы атакующего (потерянная организация защитника)
                let (rect, _) = ui.allocate_exact_size(egui::vec2(56.0, 34.0), egui::Sense::hover());
                let painter = ui.painter();
                let c = egui::pos2(rect.center().x, rect.min.y + 17.0);
                painter.circle_filled(c, 17.0, egui::Color32::from_rgb(150, 30, 30));
                painter.circle_stroke(c, 17.0, egui::Stroke::new(2.0_f32,egui::Color32::from_rgb(240, 220, 200)));
                painter.text(
                    c,
                    egui::Align2::CENTER_CENTER,
                    format!("{}%", combat.combat_progress),
                    egui::FontId::proportional(13.0),
                    egui::Color32::WHITE,
                );
                ui.label(egui::RichText::new(&terrain_text).small());
                ui.label(egui::RichText::new(&recon_text).small());
                // Организация линии: атакующие (синий) и защитники (красный), ширина фронта и резерв
                let sides = [
                    ("АТК", &att, egui::Color32::from_rgb(70, 160, 220)),
                    ("ЗАЩ", &def, egui::Color32::from_rgb(220, 90, 70)),
                ];
                for (tag, side, color) in sides {
                    ui.add(
                        egui::ProgressBar::new(side.org as f32 / side.org_max.max(1) as f32)
                            .desired_width(56.0)
                            .desired_height(5.0)
                            .fill(color),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{tag} Ширина фронта: {}/{} | В резерве: {} див.",
                            side.used_width, combat.max_width, side.reserves
                        ))
                        .small(),
                    );
                }
            });
    }
}

/// Окно выбранной дивизии: организация, прочность и статус.
fn division_info_ui(
    mut contexts: EguiContexts,
    selected: Res<SelectedDivision>,
    divisions: Query<&Division>,
    combats: Query<&ActiveCombat>,
    map: Res<ProvinceMap>,
) {
    let Some(entity) = selected.0 else { return };
    let Ok(d) = divisions.get(entity) else { return };
    let Some(ctx) = contexts.try_ctx_mut() else { return };

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
    let frame = egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 200));

    egui::Window::new("Дивизия")
        .anchor(egui::Align2::LEFT_TOP, [10.0, 10.0])
        .resizable(false)
        .collapsible(false)
        .frame(frame)
        .show(ctx, |ui| {
            ui.label(format!("Дивизия: \"{}\" [{}]", d.name, d.owner));
            // Позиция в стеке провинции (клик по провинции переключает дивизии по кругу)
            let mut stack: Vec<u32> = divisions
                .iter()
                .filter(|o| o.current_province == d.current_province && o.movement.is_none())
                .map(|o| o.id)
                .collect();
            stack.sort_unstable();
            if let Some(i) = stack.iter().position(|&id| id == d.id).filter(|_| d.movement.is_none()) {
                ui.label(format!("Дивизия {} из {} в провинции #{}", i + 1, stack.len(), d.current_province));
            }
            ui.label(format!("ID: {}", d.id));
            ui.label(format!("Провинция: {}", d.current_province));
            ui.label(format!("Организация: {}/{}", d.current_org, d.max_org));
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
                d.required_equipment
            ));
            ui.label(format!("Состояние: {status}"));
            ui.label(format!("Боевая ширина дивизии: {}", d.combat_width));
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
            .init_resource::<AirSuperiorityMap>()
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
            Division::from_template(id, "d".into(), "GER".into(), 1, PLAIN_INFANTRY_TEMPLATE.to_string(), templates.get_or_base(PLAIN_INFANTRY_TEMPLATE))
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
            Division::from_template(id, "d".into(), "GER".into(), 1, PLAIN_INFANTRY_TEMPLATE.to_string(), templates.get_or_base(PLAIN_INFANTRY_TEMPLATE))
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
        });
        let mut registry = StateRegistry::default();
        registry.province_to_state.insert(2, state);
        app.insert_resource(registry);
        app.insert_resource(AirSuperiorityMap { air_superiority: BTreeMap::from([(7, air_value)]) });
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
        Division::from_template(1, "d".into(), "GER".into(), 1, template.to_string(), templates.get_or_base(template))
    }

    #[test]
    fn armor_halves_damage_and_amplifies_return_fire() {
        let infantry = division(BASE_TEMPLATE); // броня 0, пробитие 4
        let panzer = division("Panzer-Division"); // броня 11, пробитие 16
        // пехота не пробивает броню (11 > 4): её удар по танкам вдвое слабее
        assert_eq!(apply_armor_rule(1000, &infantry, &panzer), 500);
        // танки отвечают на 50% сильнее (11 > 4)
        assert_eq!(apply_armor_rule(1000, &panzer, &infantry), 1500);
        // пехота против пехоты и танки против танков (11 < 16) — без изменений
        assert_eq!(apply_armor_rule(1000, &infantry, &infantry), 1000);
        assert_eq!(apply_armor_rule(1000, &panzer, &panzer), 1000);
    }

    #[test]
    fn armor_rule_applies_when_both_sides_resist() {
        // Сильная броня у обеих сторон и слабое пробитие: урон и режется, и усиливается (1000 / 2 * 1.5 = 750)
        let mut a = division(BASE_TEMPLATE);
        let mut b = division(BASE_TEMPLATE);
        (a.armor, a.piercing) = (50, 10);
        (b.armor, b.piercing) = (50, 10);
        assert_eq!(apply_armor_rule(1000, &a, &b), 750);
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
        Division::from_template(1, "d".into(), "GER".into(), 1, template.to_string(), templates.get_or_base(template))
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
        assert_eq!(parts(m("urban")), (30, 20, 0));
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
        Division::from_template(1, "d".into(), "GER".into(), 1, template.to_string(), templates.get_or_base(template))
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
        let side = |recon_sum, active| SideStats { used_width: 0, reserves: 0, org: 0, org_max: 0, recon_sum, active };
        assert_eq!(recon_text(3, 1), "3.0");
        assert_eq!(recon_text(5, 2), "2.5");
        assert_eq!(recon_text(0, 0), "0.0");
        assert_eq!(recon_air_label(&side(9, 3), &side(4, 4), 40), "Разведка: 3.0 vs 1.0 | Небо: Германия +40");
        assert_eq!(recon_air_label(&side(1, 1), &side(1, 1), -15), "Разведка: 1.0 vs 1.0 | Небо: Франция +15");
        assert_eq!(recon_air_label(&side(1, 1), &side(1, 1), 0), "Разведка: 1.0 vs 1.0 | Небо: паритет");
    }
}

#[cfg(test)]
mod support_tests {
    use super::*;

    fn division(template: &str) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(1, "d".into(), "GER".into(), 1, template.to_string(), templates.get_or_base(template))
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
        Division::from_template(1, "d".into(), "GER".into(), 1, template.name.clone(), template)
    }

    fn registered(name: &str) -> Division {
        let templates = DivisionTemplates::default();
        Division::from_template(1, "d".into(), "GER".into(), 1, name.to_string(), templates.get_or_base(name))
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
