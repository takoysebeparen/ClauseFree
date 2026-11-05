//! Дивизии: спавн, выбор (ЛКМ), приказы на марш по графу провинций (ПКМ),
//! движение и бой по часовым тикам `GameClock`.
//!
//! Ввод только формирует событие `OrderMove`; логика (`movement`, `current_province`, бой)
//! меняется в `apply_orders`, `advance_marches` и `resolve_combats` целыми числами по `HourTick`.
//! Позиции, масштаб спрайтов и UI-полоски — чисто визуальные и на симуляцию не влияют.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashSet};

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts};

use crate::camera::MainCamera;
use crate::clock::{ClockSet, HourTick};
use crate::map::{
    country_color, pick_province, setup_map, HoveredProvince, ProvinceAdjacency, ProvinceCenters, ProvinceMap,
    StateRegistry,
};

/// Тестовые дивизии: (id дивизии, провинция, soft_attack, defense).
/// 6521 — Берлин, 11506 — Париж (victory points).
const TEST_DIVISIONS: [(u32, u32, u32, u32); 2] = [(1, 6521, 130, 80), (2, 11506, 120, 80)];
/// Тестовый противник: id, тег владельца, soft_attack, defense. Ставится в соседнюю с Берлином провинцию.
const ENEMY_DIVISION: (u32, &str, u32, u32) = (3, "FRA", 120, 80);
const ENEMY_NEXT_TO_PROVINCE: u32 = 6521;

const BASE_ORG: u32 = 1000;
const BASE_STRENGTH: u32 = 1000;

const DIVISION_SIZE: Vec2 = Vec2::new(24.0, 14.0);
const FRAME_COLOR: Color = Color::srgb(0.9, 0.9, 0.86);
const SELECTED_FRAME_COLOR: Color = Color::srgb(1.0, 0.85, 0.1);
const FILL_COLOR: Color = Color::srgb(0.14, 0.16, 0.2);
const DIVISION_Z: f32 = 10.0;
/// Игровых часов на переход в соседнюю провинцию.
const MARCH_HOURS: u32 = 24;
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
}

impl Division {
    pub fn new(id: u32, owner: String, province: u32, soft_attack: u32, defense: u32) -> Self {
        Self {
            id,
            owner,
            current_province: province,
            movement: None,
            path: Vec::new(),
            max_org: BASE_ORG,
            current_org: BASE_ORG,
            max_strength: BASE_STRENGTH,
            current_strength: BASE_STRENGTH,
            soft_attack,
            defense,
        }
    }
}

/// Идущий бой. Отдельная сущность, существует до победы одной из сторон.
#[derive(Component, Debug, Clone)]
pub struct ActiveCombat {
    pub province_id: u32,
    pub attacker: Entity,
    pub defender: Entity,
    /// Процент потерянной защитником организации, 0..=100.
    pub combat_progress: u32,
    /// Сколько игровых часов длится бой (детерминированный источник «случайности» для отступления).
    pub elapsed_hours: u32,
}

#[derive(Resource, Default)]
pub struct SelectedDivision(pub Option<Entity>);

/// Приказ на перемещение дивизии в соседнюю провинцию.
#[derive(Event, Debug, Clone, Copy)]
pub struct OrderMove {
    pub division: Entity,
    pub target: u32,
}

// ---------------------------------------------------------------------------
// Плагин
// ---------------------------------------------------------------------------

pub struct DivisionPlugin;

impl Plugin for DivisionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SelectedDivision>()
            .add_event::<OrderMove>()
            // adjacency вставляется командами из setup_map, поэтому спавн строго после него
            .add_systems(Startup, spawn_test_divisions.after(setup_map))
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
                    .after(ClockSet),
            )
            .add_systems(Update, (combat_ui, division_info_ui));
    }
}

// ---------------------------------------------------------------------------
// Спавн
// ---------------------------------------------------------------------------

fn spawn_division_entity(commands: &mut Commands, division: Division, center: Vec2) {
    let [r, g, b] = country_color(&division.owner);
    let (id, owner, province) = (division.id, division.owner.clone(), division.current_province);

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
    println!("[ДИВИЗИЯ] #{id} ({owner}) создана в провинции {province}");
}

fn is_water(map: &ProvinceMap, province: u32) -> bool {
    matches!(map.terrain_of(province), Some("ocean" | "lakes"))
}

fn spawn_test_divisions(
    mut commands: Commands,
    centers: Res<ProvinceCenters>,
    registry: Res<StateRegistry>,
    adjacency: Res<ProvinceAdjacency>,
    map: Res<ProvinceMap>,
) {
    for (id, province, soft_attack, defense) in TEST_DIVISIONS {
        let Some(center) = centers.centers.get(&province) else {
            eprintln!("[ПРЕДУПРЕЖДЕНИЕ] нет центра для провинции {province}, дивизия {id} не создана");
            continue;
        };
        let Some(state) = registry.state_of(province) else {
            eprintln!("[ПРЕДУПРЕЖДЕНИЕ] провинция {province} без стейта, дивизия {id} не создана");
            continue;
        };
        let division = Division::new(id, state.owner.clone(), province, soft_attack, defense);
        spawn_division_entity(&mut commands, division, *center);
    }

    // Противник для проверки боя: в первой (по возрастанию id) сухопутной соседке Берлина.
    let (id, owner, soft_attack, defense) = ENEMY_DIVISION;
    let neighbor = adjacency
        .neighbors
        .get(&ENEMY_NEXT_TO_PROVINCE)
        .and_then(|set| set.iter().copied().find(|&n| !is_water(&map, n)));
    match neighbor.and_then(|n| centers.centers.get(&n).map(|c| (n, *c))) {
        Some((province, center)) => {
            let division = Division::new(id, owner.to_string(), province, soft_attack, defense);
            spawn_division_entity(&mut commands, division, center);
            println!("[ТЕСТ] Вражеская дивизия #{id} ({owner}) стоит в провинции {province} — соседе Берлина");
        }
        None => eprintln!("[ПРЕДУПРЕЖДЕНИЕ] не найден сосед Берлина для тестового противника"),
    }
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
    if !mouse.just_pressed(MouseButton::Left) || contexts.ctx_mut().wants_pointer_input() {
        return;
    }
    // При нескольких дивизиях в одной провинции берём с наименьшим id — детерминированно.
    selected.0 = hovered.id.and_then(|province| {
        divisions
            .iter()
            .filter(|(_, d)| d.current_province == province)
            .min_by_key(|(_, d)| d.id)
            .map(|(e, _)| e)
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
    if contexts.ctx_mut().wants_pointer_input() {
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

/// Евклидово расстояние, округлённое вверх (стоимость ребра g).
/// Пара floor (h) / ceil (g) сохраняет согласованность эвристики после округления.
fn dist_ceil(a: (i64, i64), b: (i64, i64)) -> u32 {
    let n = dist_sq(a, b);
    let r = isqrt(n);
    (if r * r < n { r + 1 } else { r }) as u32
}

/// A* от `start` к `goal` по `ProvinceAdjacency`; только суша. Возвращает путь включая обе точки.
/// Детерминизм: соседи идут по возрастанию id (BTreeSet), очередь — `BinaryHeap` с ключом
/// (f, id узла, g), все карты — `BTreeMap`; никаких хэш-коллекций и `f32` в расчёте.
/// Отличие от заданной сигнатуры: добавлен `map` — из него берётся тип местности (вода).
pub fn find_path_a_star(
    start: u32,
    goal: u32,
    adjacency: &ProvinceAdjacency,
    centers: &ProvinceCenters,
    map: &ProvinceMap,
) -> Option<Vec<u32>> {
    if start == goal {
        return Some(vec![start]);
    }
    if is_water(map, goal) {
        return None;
    }
    let goal_pos = center_i(centers, goal)?;
    center_i(centers, start)?;

    let mut open: BinaryHeap<Reverse<(u32, u32, u32)>> = BinaryHeap::new(); // (f, node, g)
    let mut g_score: BTreeMap<u32, u32> = BTreeMap::new();
    let mut came_from: BTreeMap<u32, u32> = BTreeMap::new();

    g_score.insert(start, 0);
    open.push(Reverse((dist_floor(center_i(centers, start)?, goal_pos), start, 0)));

    while let Some(Reverse((_, node, g))) = open.pop() {
        if g_score.get(&node).is_some_and(|&best| g > best) {
            continue; // устаревшая запись
        }
        if node == goal {
            let mut path = vec![goal];
            let mut cur = goal;
            while let Some(&prev) = came_from.get(&cur) {
                path.push(prev);
                cur = prev;
            }
            path.reverse();
            return Some(path);
        }
        let Some(node_pos) = center_i(centers, node) else { continue };
        let Some(neighbors) = adjacency.neighbors.get(&node) else { continue };
        for &n in neighbors {
            if is_water(map, n) {
                continue;
            }
            let Some(n_pos) = center_i(centers, n) else { continue };
            let tentative = g.saturating_add(dist_ceil(node_pos, n_pos));
            if g_score.get(&n).is_none_or(|&best| tentative < best) {
                g_score.insert(n, tentative);
                came_from.insert(n, node);
                open.push(Reverse((tentative.saturating_add(dist_floor(n_pos, goal_pos)), n, tentative)));
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
) {
    for order in events.read() {
        if combats.iter().any(|c| c.attacker == order.division || c.defender == order.division) {
            println!("[ПРИКАЗ] Дивизия в бою — приказ отклонён");
            continue;
        }
        let Ok(mut division) = divisions.get_mut(order.division) else { continue };

        // Если дивизия уже в пути, новый маршрут строится от точки, куда она сейчас идёт.
        let start = division.movement.map_or(division.current_province, |m| m.to);
        match find_path_a_star(start, order.target, &adjacency, &centers, &map) {
            Some(path) => {
                division.path = path.into_iter().skip(1).collect(); // без стартовой точки
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
    combats: Query<&ActiveCombat>,
    adjacency: Res<ProvinceAdjacency>,
) {
    for _ in ticks.read() {
        // Дивизии в бою (участвуют в уже существующих боях или созданных на этом тике).
        let mut busy: HashSet<Entity> = combats.iter().flat_map(|c| [c.attacker, c.defender]).collect();

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
            let enemies = snapshots.iter().filter(|s| s.province == m.to && s.owner != division.owner && s.stationary);
            let mut free_enemy = None;
            let mut any_enemy = false;
            for s in enemies {
                any_enemy = true;
                if !busy.contains(&s.entity) {
                    free_enemy = Some(s.entity);
                    break;
                }
            }

            if let Some(defender) = free_enemy {
                busy.insert(entity);
                busy.insert(defender);
                commands.spawn(ActiveCombat {
                    province_id: m.to,
                    attacker: entity,
                    defender,
                    combat_progress: 0,
                    elapsed_hours: 0,
                });
                println!("[БОЙ] Дивизия #{} атакует провинцию {}", division.id, m.to);
                continue; // маршрут приостановлен до исхода боя
            }
            if any_enemy {
                continue; // враг есть, но занят другим боем — ждём следующего тика
            }
            division.current_province = m.to;
            division.movement = None;
            if let Some(s) = snapshots.iter_mut().find(|s| s.entity == entity) {
                s.province = m.to;
                s.stationary = true;
            }
            println!("[МАРШ] Дивизия #{} прибыла в провинцию {}", division.id, m.to);
            }

            // Дивизия свободна: берём следующую точку маршрута и начинаем переход.
            if division.movement.is_none() && !division.path.is_empty() {
                let next = division.path.remove(0);
                let reachable = adjacency
                    .neighbors
                    .get(&division.current_province)
                    .is_some_and(|set| set.contains(&next));
                if reachable {
                    division.movement = Some(MovementProgress {
                        from: division.current_province,
                        to: next,
                        hours_left: MARCH_HOURS,
                        total_hours: MARCH_HOURS,
                    });
                    if let Some(s) = snapshots.iter_mut().find(|s| s.entity == entity) {
                        s.stationary = false;
                    }
                } else {
                    division.path.clear(); // маршрут устарел (например, дивизию отбросили)
                }
            }
        }
    }
}

/// Урон организации от `att` по `def` за час (целые числа).
fn org_damage(att: &Division, def: &Division) -> u32 {
    let base = att.soft_attack * 10 * att.current_strength / att.max_strength.max(1);
    (base / (10 + def.defense / 2)).max(1)
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

/// Бой на каждом часовом тике: стороны одновременно наносят урон организации и прочности.
/// Побеждает сторона, чья организация осталась > 0; при одновременном обнулении выигрывает защитник.
fn resolve_combats(
    mut ticks: EventReader<HourTick>,
    mut commands: Commands,
    mut combats: Query<(Entity, &mut ActiveCombat)>,
    mut divisions: Query<(Entity, &mut Division)>,
    mut selected: ResMut<SelectedDivision>,
    adjacency: Res<ProvinceAdjacency>,
    map: Res<ProvinceMap>,
    registry: Res<StateRegistry>,
) {
    let mut finished: HashSet<Entity> = HashSet::new();

    for _ in ticks.read() {
        // Детерминированный порядок: по провинции, затем по id атакующего.
        let mut list: Vec<(u32, u32, Entity)> = Vec::new();
        for (ce, c) in combats.iter() {
            if finished.contains(&ce) {
                continue;
            }
            match (divisions.get(c.attacker), divisions.get(c.defender)) {
                (Ok((_, a)), Ok(_)) => list.push((c.province_id, a.id, ce)),
                _ => {
                    // Одна из сторон исчезла — бой снимается, оставшийся атакующий повторит попытку на границе.
                    commands.entity(ce).despawn();
                    finished.insert(ce);
                }
            }
        }
        list.sort_unstable();

        for (_, _, ce) in list {
            let Ok((_, c)) = combats.get(ce) else { continue };
            let (province, att_e, def_e) = (c.province_id, c.attacker, c.defender);

            let (progress, outcome) = {
                let Ok([(_, mut a), (_, mut d)]) = divisions.get_many_mut([att_e, def_e]) else {
                    commands.entity(ce).despawn();
                    finished.insert(ce);
                    continue;
                };
                let to_def = org_damage(&a, &d);
                let to_att = org_damage(&d, &a);
                d.current_org = d.current_org.saturating_sub(to_def);
                d.current_strength = d.current_strength.saturating_sub((to_def / 5).max(1));
                a.current_org = a.current_org.saturating_sub(to_att);
                a.current_strength = a.current_strength.saturating_sub((to_att / 5).max(1));

                let progress = ((d.max_org - d.current_org) * 100 / d.max_org.max(1)).min(100);
                let outcome = if a.current_org == 0 {
                    Outcome::AttackerLoses
                } else if d.current_org == 0 {
                    Outcome::AttackerWins
                } else {
                    Outcome::Continue
                };
                (progress, outcome)
            };

            let elapsed = match combats.get_mut(ce) {
                Ok((_, mut c)) => {
                    c.combat_progress = progress;
                    c.elapsed_hours += 1;
                    c.elapsed_hours
                }
                Err(_) => continue,
            };

            match outcome {
                Outcome::Continue => {}
                Outcome::AttackerLoses => {
                    // Атакующий откатывается в провинцию отправления (current_province не менялся).
                    if let Ok((_, mut a)) = divisions.get_mut(att_e) {
                        a.movement = None;
                        a.path.clear(); // атака отбита — маршрут отменён
                        println!("[БОЙ] Атака на провинцию {province} отбита, дивизия #{} отступила", a.id);
                    }
                    commands.entity(ce).despawn();
                    finished.insert(ce);
                }
                Outcome::AttackerWins => {
                    // Снимок расположения всех дивизий для выбора пути отступления.
                    let occupancy: Vec<(Entity, String, u32)> =
                        divisions.iter().map(|(e, d)| (e, d.owner.clone(), d.current_province)).collect();
                    let (def_owner, def_province, def_id) = match divisions.get(def_e) {
                        Ok((_, d)) => (d.owner.clone(), d.current_province, d.id),
                        Err(_) => continue,
                    };
                    let att_from = divisions.get(att_e).ok().and_then(|(_, a)| a.movement.map(|m| m.from));

                    // Доступные: суша, не откуда пришёл атакующий, без дивизий противника защитника.
                    // Порядок — по возрастанию id (BTreeSet), поэтому выбор детерминирован.
                    let available: Vec<u32> = adjacency
                        .neighbors
                        .get(&def_province)
                        .map(|set| {
                            set.iter()
                                .copied()
                                .filter(|&n| Some(n) != att_from && !is_water(&map, n))
                                .filter(|&n| {
                                    !occupancy.iter().any(|(e, o, p)| *p == n && *e != def_e && *o != def_owner)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    // Предпочтительно суверенные провинции (того же владельца, что и защитник);
                    // если таких нет — любая доступная.
                    let sovereign: Vec<u32> = available
                        .iter()
                        .copied()
                        .filter(|&n| registry.state_of(n).is_some_and(|s| s.owner == def_owner))
                        .collect();
                    let candidates = if sovereign.is_empty() { available } else { sovereign };

                    if candidates.is_empty() {
                        println!("[БОЙ] Дивизия #{def_id} разгромлена: отступать некуда");
                        commands.entity(def_e).despawn_recursive();
                        if selected.0 == Some(def_e) {
                            selected.0 = None;
                        }
                    } else {
                        let idx = (det_rand(province as u64, def_id as u64, elapsed as u64) % candidates.len() as u64)
                            as usize;
                        let retreat = candidates[idx];
                        if let Ok((_, mut d)) = divisions.get_mut(def_e) {
                            d.current_province = retreat;
                            d.movement = None;
                            d.path.clear();
                        }
                        println!("[БОЙ] Дивизия #{def_id} отступила из {province} в провинцию {retreat}");
                    }

                    if let Ok((_, mut a)) = divisions.get_mut(att_e) {
                        a.current_province = province;
                        a.movement = None;
                        println!("[БОЙ] Дивизия #{} заняла провинцию {province}", a.id);
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

/// Маркер боя над провинцией: круглый бейдж с процентом и тонкие полоски организации сторон.
fn combat_ui(
    mut contexts: EguiContexts,
    combats: Query<(Entity, &ActiveCombat)>,
    divisions: Query<&Division>,
    centers: Res<ProvinceCenters>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    let Ok((camera, cam_tf)) = cameras.get_single() else { return };
    let ctx = contexts.ctx_mut();

    for (ce, combat) in &combats {
        let Some(center) = centers.centers.get(&combat.province_id) else { continue };
        let Some(screen) = camera.world_to_viewport(cam_tf, center.extend(0.0)) else { continue };
        let (Ok(att), Ok(def)) = (divisions.get(combat.attacker), divisions.get(combat.defender)) else {
            continue;
        };

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
                // Организация: атакующий (синий) и защитник (красный)
                for (d, color) in [(att, egui::Color32::from_rgb(70, 160, 220)), (def, egui::Color32::from_rgb(220, 90, 70))] {
                    ui.add(
                        egui::ProgressBar::new(d.current_org as f32 / d.max_org.max(1) as f32)
                            .desired_width(56.0)
                            .desired_height(5.0)
                            .fill(color),
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
) {
    let Some(entity) = selected.0 else { return };
    let Ok(d) = divisions.get(entity) else { return };
    let ctx = contexts.ctx_mut();

    let status = match combats.iter().find(|c| c.attacker == entity || c.defender == entity) {
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
            ui.label(format!("Дивизия #{} ({})", d.id, d.owner));
            ui.label(format!("Провинция: {}", d.current_province));
            ui.label(format!("Организация: {}/{}", d.current_org, d.max_org));
            ui.label(format!("Прочность: {}/{}", d.current_strength, d.max_strength));
            ui.label(format!("Атака: {} | Защита: {}", d.soft_attack, d.defense));
            ui.label(format!("Состояние: {status}"));
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
