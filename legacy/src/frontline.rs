//! Линии фронта: поиск приграничных провинций и автоматическое распределение дивизий.
//!
//! Фронт создаётся командой `CreateFrontline`; дивизии отправляются на рубеж обычными приказами
//! `OrderMove` (маршрут строит A* в `division.rs`). Порядок везде детерминирован: провинции —
//! по возрастанию id (`BTreeMap`), дивизии — по возрастанию id.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::clock::{ClockSet, DailyTickEvent};
use crate::colors::CountryColors;
use crate::command::Commander;
use crate::diplomacy::DiplomacyRegistry;
use crate::division::{
    ActiveCombat, Division, OrderMove, MAX_PLANNING, PLANNING_DECAY_PER_DAY, PLANNING_GAIN_PER_DAY,
};
use crate::economy::PlayerCountry;
use crate::map::{ProvinceAdjacency, ProvinceCenters, ProvinceControllerChanged, ProvinceControllers};

/// Противник, против которого разворачивается фронт по кнопке.
const DEFAULT_ENEMY: &str = "FRA";

// ---------------------------------------------------------------------------
// Данные
// ---------------------------------------------------------------------------

/// Линия фронта `owner` против `enemy`.
#[derive(Component, Debug, Clone)]
pub struct Frontline {
    pub owner: String,
    pub enemy: String,
    /// Приграничные провинции `owner`, по возрастанию id.
    pub provinces: Vec<u32>,
    /// Дивизии, назначенные на участки; i-я дивизия стоит на `provinces[i % provinces.len()]`.
    pub assigned_divisions: Vec<Entity>,
    /// Командующий фронтом (сущность с `Commander`). Дивизии сверх его лимита воюют вдвое хуже.
    pub commander: Option<Entity>,
}

/// Команда: общее наступление по фронту.
#[derive(Event, Debug, Clone, Copy)]
pub struct ExecuteFrontlineAttack {
    pub frontline_entity: Entity,
}

/// Команда: развернуть фронт `owner` против `enemy`.
#[derive(Event, Debug, Clone)]
pub struct CreateFrontline {
    pub owner: String,
    pub enemy: String,
}

// ---------------------------------------------------------------------------
// Алгоритмы
// ---------------------------------------------------------------------------

/// Провинции под контролем `owner`, граничащие хотя бы с одной провинцией под контролем `enemy`.
/// `ProvinceControllers` — `BTreeMap`, поэтому результат уже отсортирован по id.
pub fn get_border_provinces(
    owner: &str,
    enemy: &str,
    controllers: &ProvinceControllers,
    adjacency: &ProvinceAdjacency,
) -> Vec<u32> {
    controllers
        .0
        .iter()
        .filter(|(_, controller)| controller.as_str() == owner)
        .filter(|(id, _)| {
            adjacency.neighbors.get(id).is_some_and(|neighbors| {
                neighbors.iter().any(|n| controllers.0.get(n).is_some_and(|c| c.as_str() == enemy))
            })
        })
        .map(|(&id, _)| id)
        .collect()
}

/// Round-robin раздаёт дивизии по участкам фронта и отправляет на марш тех, кто стоит не на своём.
/// `free` — (id дивизии, сущность, текущая провинция), уже отсортированные по id.
/// Возвращает число отправленных приказов.
pub fn assign_divisions_to_frontline(
    frontline: &mut Frontline,
    free: &[(u32, Entity, u32)],
    orders: &mut EventWriter<OrderMove>,
) -> usize {
    frontline.assigned_divisions.clear();
    if frontline.provinces.is_empty() {
        return 0;
    }
    let mut sent = 0;
    for (slot, &(_, entity, current)) in free.iter().enumerate() {
        let target = frontline.provinces[slot % frontline.provinces.len()];
        frontline.assigned_divisions.push(entity);
        if current != target {
            // Путь строит A* в apply_orders (division.rs)
            orders.send(OrderMove { division: entity, target });
            sent += 1;
        }
    }
    sent
}

// ---------------------------------------------------------------------------
// Плагин и системы
// ---------------------------------------------------------------------------

pub struct FrontlinePlugin;

impl Plugin for FrontlinePlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<CreateFrontline>()
            .add_event::<ExecuteFrontlineAttack>()
            .add_systems(Update, (create_frontlines, execute_frontline_attacks, refresh_frontlines, frontline_panel))
            .add_systems(Update, daily_planning.after(ClockSet));
    }
}

/// Новое значение планирования за сутки (целые числа):
/// - дивизия не назначена на фронт (снята или фронт удалён) — сброс в 0;
/// - в бою или на марше (в том числе при наличии маршрута) — расход `PLANNING_DECAY_PER_DAY`;
/// - стоит на пограничной провинции рубежа — прирост `PLANNING_GAIN_PER_DAY` до `MAX_PLANNING`
///   (вдвое меньше, если дивизия сверх лимита командира);
/// - назначена, но ещё в тылу — без изменений.
fn next_planning(current: u32, assigned: bool, busy: bool, on_border: bool, over_limit: bool) -> u32 {
    if !assigned {
        0
    } else if busy {
        current.saturating_sub(PLANNING_DECAY_PER_DAY)
    } else if on_border {
        let gain = if over_limit { PLANNING_GAIN_PER_DAY * 50 / 100 } else { PLANNING_GAIN_PER_DAY };
        (current + gain).min(MAX_PLANNING)
    } else {
        current
    }
}

/// Суточное накопление и расход планирования.
fn daily_planning(
    mut events: EventReader<DailyTickEvent>,
    frontlines: Query<&Frontline>,
    mut divisions: Query<(Entity, &mut Division)>,
    combats: Query<&ActiveCombat>,
) {
    for _ in events.read() {
        for (entity, mut d) in &mut divisions {
            let front = frontlines.iter().find(|f| f.owner == d.owner && f.assigned_divisions.contains(&entity));
            let busy = d.movement.is_some() || !d.path.is_empty() || combats.iter().any(|c| c.involves(entity));
            let on_border = front.is_some_and(|f| f.provinces.binary_search(&d.current_province).is_ok());
            let next = next_planning(d.planning, front.is_some(), busy, on_border, d.over_command_limit);
            if next != d.planning {
                d.planning = next;
            }
        }
    }
}

/// Смена контроля над провинциями сдвигает рубеж: границы пересчитываются, а свободные дивизии,
/// оставшиеся в тылу, выходят на наименее занятые участки нового рубежа (при равенстве — ближайшие).
#[allow(clippy::too_many_arguments)]
fn refresh_frontlines(
    mut events: EventReader<ProvinceControllerChanged>,
    mut frontlines: Query<&mut Frontline>,
    controllers: Res<ProvinceControllers>,
    adjacency: Res<ProvinceAdjacency>,
    centers: Res<ProvinceCenters>,
    divisions: Query<(Entity, &Division)>,
    combats: Query<&ActiveCombat>,
    mut orders: EventWriter<OrderMove>,
) {
    if events.read().count() == 0 {
        return;
    }
    let center = |province: u32| {
        centers.centers.get(&province).map_or((0i64, 0i64), |c| (c.x.round() as i64, c.y.round() as i64))
    };

    // Детерминированный порядок обработки фронтов
    let mut list: Vec<Mut<Frontline>> = frontlines.iter_mut().collect();
    list.sort_by(|a, b| (&a.owner, &a.enemy).cmp(&(&b.owner, &b.enemy)));

    for frontline in &mut list {
        let new_provinces = get_border_provinces(&frontline.owner, &frontline.enemy, &controllers, &adjacency);
        if new_provinces == frontline.provinces {
            continue;
        }
        frontline.provinces = new_provinces;
        frontline.assigned_divisions.retain(|&e| divisions.get(e).is_ok());
        if frontline.provinces.is_empty() {
            continue;
        }

        // Загрузка участков: сколько назначенных дивизий стоит там или идёт туда
        let mut load: BTreeMap<u32, u32> = frontline.provinces.iter().map(|&p| (p, 0)).collect();
        let mut rear: Vec<(u32, Entity, u32)> = Vec::new();
        for &entity in &frontline.assigned_divisions {
            let Ok((_, d)) = divisions.get(entity) else { continue };
            let destination = d.path.last().copied().or(d.movement.map(|m| m.to)).unwrap_or(d.current_province);
            if let Some(count) = load.get_mut(&destination) {
                *count += 1;
            }
            let idle = d.movement.is_none()
                && d.path.is_empty()
                && !combats.iter().any(|c| c.involves(entity));
            if idle && frontline.provinces.binary_search(&d.current_province).is_err() {
                rear.push((d.id, entity, d.current_province));
            }
        }
        rear.sort_unstable();

        let mut sent = 0;
        for (_, entity, current) in rear {
            let (cx, cy) = center(current);
            let best = frontline.provinces.iter().copied().min_by_key(|&p| {
                let (px, py) = center(p);
                (load[&p], (px - cx) * (px - cx) + (py - cy) * (py - cy), p)
            });
            if let Some(target) = best {
                orders.send(OrderMove { division: entity, target });
                *load.entry(target).or_default() += 1;
                sent += 1;
            }
        }
        println!(
            "[ФРОНТ] {} vs {}: рубеж сдвинут, участков {}, дивизий из тыла отправлено {sent}",
            frontline.owner,
            frontline.enemy,
            frontline.provinces.len()
        );
    }
}

/// Дивизия готова к наступлению: стоит на рубеже, свободна от марша, маршрута и боя.
fn is_ready(entity: Entity, d: &Division, frontline: &Frontline, combats: &Query<&ActiveCombat>) -> bool {
    d.movement.is_none()
        && d.path.is_empty()
        && frontline.provinces.binary_search(&d.current_province).is_ok() // provinces отсортированы
        && !combats.iter().any(|c| c.involves(entity))
}

/// Общее наступление: каждая готовая дивизия атакует смежную провинцию противника, в которую
/// нацелено меньше всего союзников (при равенстве — с меньшим id). Порядок — по id дивизии.
fn execute_frontline_attacks(
    mut events: EventReader<ExecuteFrontlineAttack>,
    frontlines: Query<&Frontline>,
    divisions: Query<(Entity, &Division)>,
    combats: Query<&ActiveCombat>,
    controllers: Res<ProvinceControllers>,
    adjacency: Res<ProvinceAdjacency>,
    mut orders: EventWriter<OrderMove>,
    mut diplomacy: Option<ResMut<DiplomacyRegistry>>,
) {
    for event in events.read() {
        let Ok(frontline) = frontlines.get(event.frontline_entity) else { continue };
        if let Some(diplomacy) = diplomacy.as_deref_mut() {
            if diplomacy.declare_war(&frontline.owner, &frontline.enemy) {
                println!("[ДИПЛОМАТИЯ] {} объявила войну {}", frontline.owner, frontline.enemy);
            }
        }

        // Сколько своих дивизий уже идёт в каждую вражескую провинцию
        let mut load: BTreeMap<u32, u32> = BTreeMap::new();
        for (_, d) in divisions.iter().filter(|(_, d)| d.owner == frontline.owner) {
            let target = d.path.last().copied().or(d.movement.map(|m| m.to));
            if let Some(t) = target.filter(|t| controllers.0.get(t).is_some_and(|c| *c == frontline.enemy)) {
                *load.entry(t).or_default() += 1;
            }
        }

        let mut ready: Vec<(u32, Entity, u32)> = frontline
            .assigned_divisions
            .iter()
            .filter_map(|&e| divisions.get(e).ok())
            .filter(|&(e, d)| is_ready(e, d, frontline, &combats))
            .map(|(e, d)| (d.id, e, d.current_province))
            .collect();
        ready.sort_unstable();

        let mut sent = 0u32;
        for (_, entity, province) in ready {
            let target = adjacency
                .neighbors
                .get(&province)
                .into_iter()
                .flatten()
                .copied()
                .filter(|n| controllers.0.get(n).is_some_and(|c| *c == frontline.enemy))
                .min_by_key(|&n| (load.get(&n).copied().unwrap_or(0), n));
            if let Some(target) = target {
                orders.send(OrderMove { division: entity, target });
                *load.entry(target).or_default() += 1;
                sent += 1;
            }
        }

        if sent > 0 {
            println!("[НАСТУПЛЕНИЕ] Фронт {} vs {}: {sent} дивизий перешли в атаку!", frontline.owner, frontline.enemy);
        } else {
            println!("[НАСТУПЛЕНИЕ] Фронт {} vs {}: нет готовых дивизий", frontline.owner, frontline.enemy);
        }
    }
}

/// Строит `Frontline` для пары стран и отправляет свободные дивизии `owner` на рубеж.
/// Возвращает фронт и число дивизий, отправленных на марш, либо причину отказа.
/// Общая логика для команды игрока (`CreateFrontline`) и для ИИ.
/// Фронту автоматически назначается лучший свободный командир страны.
#[allow(clippy::too_many_arguments)]
pub fn build_frontline(
    owner: &str,
    enemy: &str,
    controllers: &ProvinceControllers,
    adjacency: &ProvinceAdjacency,
    divisions: &Query<(Entity, &Division)>,
    combats: &Query<&ActiveCombat>,
    commanders: &Query<(Entity, &Commander)>,
    frontlines: &Query<(Entity, &Frontline)>,
    orders: &mut EventWriter<OrderMove>,
) -> Result<(Frontline, usize), &'static str> {
    let provinces = get_border_provinces(owner, enemy, controllers, adjacency);
    if provinces.is_empty() {
        return Err("общей границы нет");
    }

    // Свободные дивизии: не в бою, не в марше, без маршрута. Порядок — по id.
    let mut free: Vec<(u32, Entity, u32)> = divisions
        .iter()
        .filter(|(entity, d)| {
            d.owner == owner
                && d.movement.is_none()
                && d.path.is_empty()
                && !combats.iter().any(|c| c.involves(*entity))
        })
        .map(|(entity, d)| (d.id, entity, d.current_province))
        .collect();
    free.sort_unstable();
    if free.is_empty() {
        return Err("нет свободных дивизий");
    }

    let mut frontline = Frontline {
        owner: owner.to_string(),
        enemy: enemy.to_string(),
        provinces,
        assigned_divisions: Vec::new(),
        commander: choose_commander(owner, enemy, commanders, frontlines),
    };
    let sent = assign_divisions_to_frontline(&mut frontline, &free, orders);
    Ok((frontline, sent))
}

/// Лучший свободный командир страны: по сумме навыков, при равенстве — с меньшим `id`.
/// Заняты командиры других фронтов; командир фронта той же пары (который будет заменён) доступен.
pub fn choose_commander(
    owner: &str,
    enemy: &str,
    commanders: &Query<(Entity, &Commander)>,
    frontlines: &Query<(Entity, &Frontline)>,
) -> Option<Entity> {
    let taken: HashSet<Entity> = frontlines
        .iter()
        .filter(|(_, f)| !(f.owner == owner && f.enemy == enemy))
        .filter_map(|(_, f)| f.commander)
        .collect();
    commanders
        .iter()
        .filter(|(e, c)| c.owner == owner && !taken.contains(e))
        .max_by_key(|(_, c)| (c.attack_skill + c.defense_skill, Reverse(c.id)))
        .map(|(e, _)| e)
}

/// Обработка команды `CreateFrontline`: граница, создание `Frontline`, назначение дивизий.
#[allow(clippy::too_many_arguments)]
fn create_frontlines(
    mut events: EventReader<CreateFrontline>,
    mut commands: Commands,
    existing: Query<(Entity, &Frontline)>,
    controllers: Res<ProvinceControllers>,
    adjacency: Res<ProvinceAdjacency>,
    divisions: Query<(Entity, &Division)>,
    combats: Query<&ActiveCombat>,
    commanders: Query<(Entity, &Commander)>,
    mut orders: EventWriter<OrderMove>,
    mut diplomacy: Option<ResMut<DiplomacyRegistry>>,
) {
    let mut replaced: HashSet<Entity> = HashSet::new();
    for event in events.read() {
        // Развёртывание фронта означает войну пары: только она открывает чужую территорию для маршей
        if let Some(diplomacy) = diplomacy.as_deref_mut() {
            if diplomacy.declare_war(&event.owner, &event.enemy) {
                println!("[ДИПЛОМАТИЯ] {} объявила войну {}", event.owner, event.enemy);
            }
        }
        let (frontline, sent) = match build_frontline(
            &event.owner,
            &event.enemy,
            &controllers,
            &adjacency,
            &divisions,
            &combats,
            &commanders,
            &existing,
            &mut orders,
        ) {
            Ok(built) => built,
            Err(reason) => {
                println!("[ФРОНТ] {} vs {}: {reason}", event.owner, event.enemy);
                continue;
            }
        };

        // Новый фронт той же пары заменяет прежний (два события в кадре не удаляют один фронт дважды)
        for (entity, old) in &existing {
            if old.owner == event.owner && old.enemy == event.enemy && replaced.insert(entity) {
                commands.entity(entity).despawn();
            }
        }
        println!(
            "[ФРОНТ] {} vs {}: участков {}, дивизий {}, отправлено на марш {}",
            frontline.owner,
            frontline.enemy,
            frontline.provinces.len(),
            frontline.assigned_divisions.len(),
            sent
        );
        commands.spawn(frontline);
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// Средний `planning` назначенных на фронт дивизий (целочисленное деление, 0 для пустого фронта).
fn average_planning(frontline: &Frontline, divisions: &Query<(Entity, &Division)>) -> u32 {
    let planning: Vec<u32> =
        frontline.assigned_divisions.iter().filter_map(|&e| divisions.get(e).ok()).map(|(_, d)| d.planning).collect();
    if planning.is_empty() { 0 } else { planning.iter().sum::<u32>() / planning.len() as u32 }
}

/// Панель управления внизу справа: кнопка развёртывания фронта и сводка по фронтам.
fn frontline_panel(
    mut contexts: EguiContexts,
    player: Res<PlayerCountry>,
    frontlines: Query<(Entity, &Frontline)>,
    divisions: Query<(Entity, &Division)>,
    combats: Query<&ActiveCombat>,
    commanders: Query<&Commander>,
    colors: Res<CountryColors>,
    mut commands_out: EventWriter<CreateFrontline>,
    mut attack_out: EventWriter<ExecuteFrontlineAttack>,
) {
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    let frame = egui::Frame::window(&ctx.style())
        .fill(crate::ui::PANEL_FILL)
        .stroke(egui::Stroke::new(2.0_f32, colors.ui_color32(&player.0)));

    egui::Window::new(crate::ui::window_title("Управление"))
        .anchor(egui::Align2::RIGHT_BOTTOM, [-10.0, -10.0])
        .resizable(false)
        .collapsible(false)
        .frame(frame)
        .show(ctx, |ui| {
            let label = format!("[Развернуть фронт против Франции ({} vs {DEFAULT_ENEMY})]", player.0);
            if ui.button(label).clicked() {
                commands_out.send(CreateFrontline { owner: player.0.clone(), enemy: DEFAULT_ENEMY.to_string() });
            }

            // Наступление доступно, когда фронт игрока создан и на рубеже есть готовые дивизии
            let own = frontlines.iter().find(|(_, f)| f.owner == player.0 && f.enemy == DEFAULT_ENEMY);
            let ready = own.map_or(0, |(_, f)| {
                f.assigned_divisions
                    .iter()
                    .filter_map(|&e| divisions.get(e).ok())
                    .filter(|&(e, d)| is_ready(e, d, f, &combats))
                    .count()
            });
            let attack_label = format!("[Начать наступление ({} -> {DEFAULT_ENEMY})]", player.0);
            let response = ui.add_enabled(ready > 0, egui::Button::new(attack_label));
            if response.clicked() {
                if let Some((entity, _)) = own {
                    attack_out.send(ExecuteFrontlineAttack { frontline_entity: entity });
                }
            }
            if own.is_some() {
                ui.label(format!("Готовых к наступлению дивизий: {ready}"));
            }

            for (_, f) in &frontlines {
                let avg = average_planning(f, &divisions);
                ui.horizontal(|ui| {
                    ui.label("Фронт");
                    ui.label(egui::RichText::new(&f.owner).color(colors.ui_color32(&f.owner)).strong());
                    ui.label("vs");
                    ui.label(egui::RichText::new(&f.enemy).color(colors.ui_color32(&f.enemy)).strong());
                    ui.label(format!(": участков {}, дивизий {}", f.provinces.len(), f.assigned_divisions.len()));
                });
                match f.commander.and_then(|e| commanders.get(e).ok()) {
                    Some(c) => {
                        ui.label(format!(
                            "Командующий: {} (Ранг: {}, Атк: {}, Защ: {}) | Состав: {}/{} див.",
                            c.name,
                            c.rank.title(),
                            c.attack_skill,
                            c.defense_skill,
                            f.assigned_divisions.len(),
                            c.rank.division_capacity()
                        ));
                    }
                    None => {
                        ui.label("Командующий: не назначен");
                    }
                }
                // Средний план наступления армии фронта (проценты от максимума — целочисленно)
                ui.label(format!(
                    "Готовность наступления: {avg}/{MAX_PLANNING} ({}%)",
                    avg * 100 / MAX_PLANNING
                ));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::division::MovementProgress;
    use crate::units::{DivisionTemplates, BASE_TEMPLATE};
    use std::collections::BTreeMap;

    #[test]
    fn next_planning_rules() {
        // на рубеже: +2 до потолка 30
        assert_eq!(next_planning(0, true, false, true, false), 2);
        assert_eq!(next_planning(29, true, false, true, false), 30);
        assert_eq!(next_planning(30, true, false, true, false), 30);
        // в бою / на марше: -3, не ниже нуля (даже на рубеже)
        assert_eq!(next_planning(30, true, true, true, false), 27);
        assert_eq!(next_planning(2, true, true, false, false), 0);
        // назначена, но в тылу: без изменений
        assert_eq!(next_planning(10, true, false, false, false), 10);
        // сверх лимита командира: прирост вдвое меньше (+1), расход прежний
        assert_eq!(next_planning(0, true, false, true, true), 1);
        assert_eq!(next_planning(30, true, true, true, true), 27);
        // снята с фронта: сброс
        assert_eq!(next_planning(30, false, false, true, false), 0);
        assert_eq!(next_planning(12, false, true, false, false), 0);
    }

    fn spawn_division(app: &mut App, id: u32, province: u32) -> Entity {
        let templates = DivisionTemplates::default();
        let d = Division::from_template(
            id,
            format!("Div {id}"),
            "GER".to_string(),
            province,
            BASE_TEMPLATE.to_string(),
            templates.get_or_base(BASE_TEMPLATE),
        );
        app.world_mut().spawn(d).id()
    }

    fn spawn_owned(app: &mut App, id: u32, owner: &str, province: u32) -> Entity {
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

    fn spawn_commander(app: &mut App, id: u32, owner: &str, name: &str, attack: u32, defense: u32) {
        app.world_mut().spawn(Commander {
            id,
            name: name.to_string(),
            owner: owner.to_string(),
            rank: crate::command::CommanderRank::General,
            attack_skill: attack,
            defense_skill: defense,
            assigned_divisions: Vec::new(),
        });
    }

    fn create(app: &mut App, owner: &str, enemy: &str) {
        app.world_mut().send_event(CreateFrontline { owner: owner.into(), enemy: enemy.into() });
        app.update(); // обработка команды и применение spawn
    }

    /// Имя командира фронта пары (owner, enemy).
    fn front_commander(app: &mut App, owner: &str, enemy: &str) -> Option<String> {
        let world = app.world_mut();
        let commander = world
            .query::<&Frontline>()
            .iter(world)
            .find(|f| f.owner == owner && f.enemy == enemy)
            .and_then(|f| f.commander)?;
        world.get::<Commander>(commander).map(|c| c.name.clone())
    }

    #[test]
    fn frontline_gets_best_free_general_of_its_nation() {
        let mut app = App::new();
        // 1 (GER) граничит с 2 (FRA) и 3 (ITA); у каждой стороны есть дивизия
        let neighbors = BTreeMap::from([
            (1, std::collections::BTreeSet::from([2, 3])),
            (2, std::collections::BTreeSet::from([1])),
            (3, std::collections::BTreeSet::from([1])),
        ]);
        let controllers = ProvinceControllers(BTreeMap::from([
            (1, "GER".to_string()),
            (2, "FRA".to_string()),
            (3, "ITA".to_string()),
        ]));
        app.add_event::<CreateFrontline>()
            .add_event::<OrderMove>()
            .insert_resource(ProvinceAdjacency { neighbors })
            .insert_resource(controllers)
            .add_systems(Update, create_frontlines);

        spawn_commander(&mut app, 0, "GER", "Роммель", 4, 2);
        spawn_commander(&mut app, 1, "GER", "Гудериан", 4, 1);
        spawn_commander(&mut app, 2, "FRA", "Гамелен", 1, 3);
        spawn_commander(&mut app, 3, "FRA", "де Голль", 3, 2);
        spawn_owned(&mut app, 1, "GER", 1);
        spawn_owned(&mut app, 2, "FRA", 2);

        // лучший по сумме навыков: Роммель (6) против Гудериана (5)
        create(&mut app, "GER", "FRA");
        assert_eq!(front_commander(&mut app, "GER", "FRA").as_deref(), Some("Роммель"));
        // Роммель занят — на второй фронт Германии идёт Гудериан
        create(&mut app, "GER", "ITA");
        assert_eq!(front_commander(&mut app, "GER", "ITA").as_deref(), Some("Гудериан"));
        // Франция: де Голль (5) лучше Гамелена (4)
        create(&mut app, "FRA", "GER");
        assert_eq!(front_commander(&mut app, "FRA", "GER").as_deref(), Some("де Голль"));
        // Пересоздание фронта той же пары снова берёт своего командира (а не оставляет без командира)
        create(&mut app, "GER", "FRA");
        assert_eq!(front_commander(&mut app, "GER", "FRA").as_deref(), Some("Роммель"));
    }
    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    fn planning(app: &App, e: Entity) -> u32 {
        app.world().get::<Division>(e).expect("дивизия жива").planning
    }

    #[test]
    fn daily_planning_accumulates_spends_and_resets() {
        let mut app = App::new();
        app.add_event::<DailyTickEvent>().add_systems(Update, daily_planning);

        let on_border = spawn_division(&mut app, 1, 5); // рубеж: провинция 5
        let in_rear = spawn_division(&mut app, 2, 9); // назначена, но стоит в тылу
        let unassigned = spawn_division(&mut app, 3, 5); // на фронт не назначена
        app.world_mut().get_mut::<Division>(unassigned).unwrap().planning = 10;

        let front = app
            .world_mut()
            .spawn(Frontline {
                owner: "GER".into(),
                enemy: "FRA".into(),
                provinces: vec![5, 6],
                assigned_divisions: vec![on_border, in_rear],
                commander: None,
            })
            .id();

        // Накопление: +2 в сутки до потолка 30 (15 суток), в тылу — ничего, не назначенная — сброс
        for _ in 0..10 {
            day(&mut app);
        }
        assert_eq!(planning(&app, on_border), 20);
        assert_eq!(planning(&app, in_rear), 0);
        assert_eq!(planning(&app, unassigned), 0);
        for _ in 0..10 {
            day(&mut app);
        }
        assert_eq!(planning(&app, on_border), 30, "потолок MAX_PLANNING");

        // Марш: накопление стоп, расход -3 в сутки
        app.world_mut().get_mut::<Division>(on_border).unwrap().movement =
            Some(MovementProgress { from: 5, to: 6, hours_left: 24, total_hours: 24 });
        day(&mut app);
        assert_eq!(planning(&app, on_border), 27);
        day(&mut app);
        assert_eq!(planning(&app, on_border), 24);

        // Фронт удалён — на следующих сутках планирование сбрасывается
        app.world_mut().despawn(front);
        day(&mut app);
        assert_eq!(planning(&app, on_border), 0);
    }

    #[test]
    fn deploying_a_front_and_attacking_declare_war() {
        let mut app = App::new();
        let neighbors = BTreeMap::from([(1, std::collections::BTreeSet::from([2])), (2, std::collections::BTreeSet::from([1]))]);
        app.add_event::<CreateFrontline>()
            .add_event::<ExecuteFrontlineAttack>()
            .add_event::<OrderMove>()
            .init_resource::<DiplomacyRegistry>()
            .insert_resource(ProvinceAdjacency { neighbors })
            .insert_resource(ProvinceControllers(BTreeMap::from([(1, "GER".to_string()), (2, "FRA".to_string())])))
            .add_systems(Update, create_frontlines);
        spawn_commander(&mut app, 0, "GER", "Роммель", 4, 2);
        spawn_owned(&mut app, 1, "GER", 1);
        assert!(!app.world().resource::<DiplomacyRegistry>().are_at_war("GER", "FRA"), "старт: мир");
        create(&mut app, "GER", "FRA");
        assert!(app.world().resource::<DiplomacyRegistry>().are_at_war("GER", "FRA"));
        assert!(!app.world().resource::<DiplomacyRegistry>().are_at_war("GER", "ITA"), "другие пары не затронуты");
    }
}
