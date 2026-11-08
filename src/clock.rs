//! Игровые часы: целочисленное время, пауза, скорость и часовые тики.
//!
//! Вся логика симуляции работает от события `HourTick`; реальное время (`Time`) используется
//! только для накопления микросекунд в целочисленном аккумуляторе. Управление идёт через
//! `ClockCommand` (единая точка входа для будущего мультиплеера).

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Реальные микросекунды на один игровой час для скоростей 1..=5.
const TICK_INTERVAL_US: [u64; 5] = [500_000, 250_000, 125_000, 62_500, 25_000];
/// Защита от спирали тиков при просадке FPS: лишнее накопленное время отбрасывается.
const MAX_TICKS_PER_FRAME: u32 = 8;

pub const MIN_SPEED: u8 = 1;
pub const MAX_SPEED: u8 = 5;

// ---------------------------------------------------------------------------
// Ресурс, команды, события
// ---------------------------------------------------------------------------

#[derive(Resource, Debug, Clone)]
pub struct GameClock {
    /// Абсолютный номер игрового часа (0 = старт).
    pub tick: u64,
    pub hour: u32,
    pub day: u32,
    pub month: u32,
    pub year: u32,
    pub paused: bool,
    /// 1..=5
    pub speed: u8,
    /// Накопленное реальное время в микросекундах.
    accumulator_us: u64,
}

impl Default for GameClock {
    fn default() -> Self {
        // 1 января 1936, 12:00; старт на паузе
        Self { tick: 0, hour: 12, day: 1, month: 1, year: 1936, paused: true, speed: 1, accumulator_us: 0 }
    }
}

impl GameClock {
    fn advance_hour(&mut self) {
        self.tick += 1;
        self.hour += 1;
        if self.hour < 24 {
            return;
        }
        self.hour = 0;
        self.day += 1;
        if self.day <= days_in_month(self.month, self.year) {
            return;
        }
        self.day = 1;
        self.month += 1;
        if self.month > 12 {
            self.month = 1;
            self.year += 1;
        }
    }

    pub fn date_string(&self) -> String {
        format!("{} {} {}, {:02}:00", self.day, MONTHS[(self.month - 1) as usize], self.year, self.hour)
    }
}

fn days_in_month(month: u32, year: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap { 29 } else { 28 }
        }
    }
}

/// Команды управления временем.
#[derive(Event, Debug, Clone, Copy)]
pub enum ClockCommand {
    TogglePause,
    SetSpeed(u8),
    SpeedUp,
    SpeedDown,
}

/// Один прошедший игровой час; на него реагирует вся симуляция.
#[derive(Event, Debug, Clone, Copy)]
pub struct HourTick;

/// Суточный такт: срабатывает при переходе часов с 23:00 на 00:00. Поля — дата нового дня.
/// Для тяжёлой логики (экономика), которая не должна считаться каждый час.
#[derive(Event, Debug, Clone, Copy)]
pub struct DailyTickEvent {
    pub day: u32,
    pub month: u32,
    pub year: u32,
}

/// Системы, генерирующие `HourTick`. Симуляция упорядочивается `.after(ClockSet)`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClockSet;

// ---------------------------------------------------------------------------
// Плагин
// ---------------------------------------------------------------------------

pub struct ClockPlugin;

impl Plugin for ClockPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GameClock>()
            .add_event::<ClockCommand>()
            .add_event::<HourTick>()
            .add_event::<DailyTickEvent>()
            .add_systems(
                Update,
                (clock_input, apply_clock_commands, tick_clock).chain().in_set(ClockSet),
            )
            .add_systems(Update, clock_ui.after(ClockSet));
    }
}

// ---------------------------------------------------------------------------
// Системы
// ---------------------------------------------------------------------------

fn clock_input(keys: Res<ButtonInput<KeyCode>>, mut out: EventWriter<ClockCommand>) {
    if keys.just_pressed(KeyCode::Space) {
        out.send(ClockCommand::TogglePause);
    }
    if keys.just_pressed(KeyCode::NumpadAdd) || keys.just_pressed(KeyCode::Equal) {
        out.send(ClockCommand::SpeedUp);
    }
    if keys.just_pressed(KeyCode::NumpadSubtract) || keys.just_pressed(KeyCode::Minus) {
        out.send(ClockCommand::SpeedDown);
    }
    for (key, speed) in [
        (KeyCode::Digit1, 1u8),
        (KeyCode::Digit2, 2),
        (KeyCode::Digit3, 3),
        (KeyCode::Digit4, 4),
        (KeyCode::Digit5, 5),
    ] {
        if keys.just_pressed(key) {
            out.send(ClockCommand::SetSpeed(speed));
        }
    }
}

/// Единственное место, где команды меняют паузу и скорость.
fn apply_clock_commands(mut events: EventReader<ClockCommand>, mut clock: ResMut<GameClock>) {
    for cmd in events.read() {
        match *cmd {
            ClockCommand::TogglePause => clock.paused = !clock.paused,
            ClockCommand::SetSpeed(s) => clock.speed = s.clamp(MIN_SPEED, MAX_SPEED),
            ClockCommand::SpeedUp => clock.speed = (clock.speed + 1).min(MAX_SPEED),
            ClockCommand::SpeedDown => clock.speed = clock.speed.saturating_sub(1).max(MIN_SPEED),
        }
    }
}

/// Накапливает реальное время (целые микросекунды) и выдаёт `HourTick`.
fn tick_clock(
    time: Res<Time>,
    mut clock: ResMut<GameClock>,
    mut ticks: EventWriter<HourTick>,
    mut daily: EventWriter<DailyTickEvent>,
) {
    if clock.paused {
        clock.accumulator_us = 0;
        return;
    }
    let interval = TICK_INTERVAL_US[(clock.speed.clamp(MIN_SPEED, MAX_SPEED) - 1) as usize];
    let mut acc = clock.accumulator_us + time.delta().as_micros() as u64;

    let mut emitted = 0;
    while acc >= interval && emitted < MAX_TICKS_PER_FRAME {
        acc -= interval;
        clock.advance_hour();
        ticks.send(HourTick);
        // Переход 23:00 -> 00:00: наступили новые сутки
        if clock.hour == 0 {
            daily.send(DailyTickEvent { day: clock.day, month: clock.month, year: clock.year });
        }
        emitted += 1;
    }
    if emitted == MAX_TICKS_PER_FRAME {
        acc = 0; // отстали — отбрасываем долг
    }
    clock.accumulator_us = acc;
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

fn clock_ui(mut contexts: EguiContexts, clock: Res<GameClock>) {
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    let frame = egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 200));

    egui::Window::new("clock_panel")
        .title_bar(false)
        .anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0])
        .resizable(false)
        .frame(frame)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let status = if clock.paused { "[PAUSED]" } else { "[PLAYING]" };
                ui.label(format!("{} | {} (Space)", clock.date_string(), status));
                ui.separator();

                // Индикатор скорости: активные уровни ярко, остальные приглушены
                ui.spacing_mut().item_spacing.x = 1.0;
                let active = clock.speed.clamp(MIN_SPEED, MAX_SPEED) as usize;
                let lit = if clock.paused { egui::Color32::from_gray(150) } else { egui::Color32::from_rgb(255, 215, 60) };
                ui.label(egui::RichText::new(">".repeat(active)).color(lit).strong());
                ui.label(egui::RichText::new(">".repeat(MAX_SPEED as usize - active)).color(egui::Color32::from_gray(80)));
            });
        });
}
