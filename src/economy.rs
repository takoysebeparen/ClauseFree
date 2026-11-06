//! Экономика (Фаза 5): фабрики стран, национальный склад и суточное производство.
//!
//! Всё считается целыми числами и привязано к тегу страны (`CountryEconomy`).
//! Тяжёлая логика идёт строго на `DailyTickEvent` (00:00), а не каждый час.

use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::clock::{ClockSet, DailyTickEvent};
use crate::map::StateRegistry;

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

// ---------------------------------------------------------------------------
// Плагин
// ---------------------------------------------------------------------------

pub struct EconomyPlugin;

impl Plugin for EconomyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Economies>()
            .insert_resource(PlayerCountry(DEFAULT_PLAYER_COUNTRY.to_string()))
            .add_systems(Startup, init_economies)
            .add_systems(Update, (daily_production.after(ClockSet), economy_bar));
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
