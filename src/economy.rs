//! Экономика (Фаза 5): фабрики стран, национальный склад и суточное производство.
//!
//! Всё считается целыми числами и привязано к тегу страны (`CountryEconomy`).
//! Тяжёлая логика идёт строго на `DailyTickEvent` (00:00), а не каждый час.

use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::clock::{ClockSet, DailyTickEvent};
use crate::division::Division;
use crate::map::StateRegistry;

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

// ---------------------------------------------------------------------------
// Плагин
// ---------------------------------------------------------------------------

pub struct EconomyPlugin;

impl Plugin for EconomyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Economies>()
            .insert_resource(PlayerCountry(DEFAULT_PLAYER_COUNTRY.to_string()))
            .add_systems(Startup, init_economies)
            // Суточный порядок: сначала производство на склад, затем пополнение дивизий со склада
            .add_systems(
                Update,
                (daily_production.after(ClockSet), daily_reinforcement.after(daily_production), economy_bar),
            );
    }
}

// ---------------------------------------------------------------------------
// Системы
// ---------------------------------------------------------------------------

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

/// Суточное пополнение: дивизии с неполной прочностью запрашивают до 50 единиц, склад тратит
/// столько же винтовок. При нехватке запас делится пропорционально запросам (целочисленно,
/// остаток раздаётся по одному в порядке id). Всё детерминировано: страны и дивизии — по порядку.
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
        for (entity, d) in divisions.iter() {
            let deficit = d.max_strength.saturating_sub(d.current_strength);
            if deficit > 0 {
                let strength_request = deficit.min(REINFORCE_PER_DIVISION_PER_DAY);
                let equipment_request = (u64::from(strength_request) * u64::from(d.required_equipment))
                    .div_ceil(u64::from(d.max_strength.max(1))) as u32;
                requests
                    .entry(d.owner.clone())
                    .or_default()
                    .push((d.id, entity, equipment_request, strength_request));
            }
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
