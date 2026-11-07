//! Линии фронта: поиск приграничных провинций и автоматическое распределение дивизий.
//!
//! Фронт создаётся командой `CreateFrontline`; дивизии отправляются на рубеж обычными приказами
//! `OrderMove` (маршрут строит A* в `division.rs`). Порядок везде детерминирован: провинции —
//! по возрастанию id (`BTreeMap`), дивизии — по возрастанию id.

use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::division::{ActiveCombat, Division, OrderMove};
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
            .add_systems(Update, (create_frontlines, execute_frontline_attacks, refresh_frontlines, frontline_panel));
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
) {
    for event in events.read() {
        let Ok(frontline) = frontlines.get(event.frontline_entity) else { continue };

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
pub fn build_frontline(
    owner: &str,
    enemy: &str,
    controllers: &ProvinceControllers,
    adjacency: &ProvinceAdjacency,
    divisions: &Query<(Entity, &Division)>,
    combats: &Query<&ActiveCombat>,
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
    };
    let sent = assign_divisions_to_frontline(&mut frontline, &free, orders);
    Ok((frontline, sent))
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
    mut orders: EventWriter<OrderMove>,
) {
    let mut replaced: std::collections::HashSet<Entity> = std::collections::HashSet::new();
    for event in events.read() {
        let (frontline, sent) = match build_frontline(
            &event.owner,
            &event.enemy,
            &controllers,
            &adjacency,
            &divisions,
            &combats,
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

/// Панель управления внизу справа: кнопка развёртывания фронта и сводка по фронтам.
fn frontline_panel(
    mut contexts: EguiContexts,
    player: Res<PlayerCountry>,
    frontlines: Query<(Entity, &Frontline)>,
    divisions: Query<(Entity, &Division)>,
    combats: Query<&ActiveCombat>,
    mut commands_out: EventWriter<CreateFrontline>,
    mut attack_out: EventWriter<ExecuteFrontlineAttack>,
) {
    let ctx = contexts.ctx_mut();
    let frame = egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 200));

    egui::Window::new("Управление")
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
                ui.label(format!(
                    "Фронт {} vs {}: участков {}, дивизий {}",
                    f.owner,
                    f.enemy,
                    f.provinces.len(),
                    f.assigned_divisions.len()
                ));
            }
        });
}
