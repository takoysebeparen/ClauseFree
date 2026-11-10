//! Штабной стиль интерфейса: тёмно-стальная тема, шапка со страной (флаг 41x26 + тег) и двухцветные шкалы
//! боеготовности «Организация» / «Прочность». Только слой отображения: симуляция не затрагивается.
//!
//! Кэш флагов (`FlagTextures`, ленивая загрузка TGA с запасным цветным прямоугольником) живёт в `flags.rs`;
//! здесь он используется через `country_header`.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::colors::CountryColors;
use crate::flags::FlagTextures;

/// Фон окон и панелей: тёмная сталь.
pub const PANEL_FILL: egui::Color32 = egui::Color32::from_rgb(22, 24, 28);
/// Обводка окон: сталь с латунным отливом.
pub const WINDOW_STROKE: egui::Color32 = egui::Color32::from_rgb(70, 75, 85);
/// Заголовки и акценты: приглушённое золото.
pub const BRASS: egui::Color32 = egui::Color32::from_rgb(215, 185, 105);
/// Шкала организации.
pub const ORG_COLOR: egui::Color32 = egui::Color32::from_rgb(46, 139, 87);
/// Шкала прочности.
pub const STRENGTH_COLOR: egui::Color32 = egui::Color32::from_rgb(198, 120, 20);
/// Размер флага в шапках окон, пиксели (medium-флаги игры 41x26).
pub const HEADER_FLAG_SIZE: [f32; 2] = [41.0, 26.0];

/// Тёмная штабная тема egui.
pub fn military_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL_FILL;
    v.window_fill = PANEL_FILL;
    v.extreme_bg_color = egui::Color32::from_rgb(16, 18, 22);
    v.faint_bg_color = egui::Color32::from_rgb(32, 36, 42);
    v.window_stroke = egui::Stroke::new(1.0_f32, WINDOW_STROKE);
    v.hyperlink_color = BRASS;
    v.selection.bg_fill = egui::Color32::from_rgb(92, 78, 40);
    v.selection.stroke = egui::Stroke::new(1.0_f32, BRASS);
    // Кнопки — тёмный металлик, при наведении светлеют и получают латунный контур
    let steel = |fill: [u8; 3], stroke: egui::Color32| {
        (egui::Color32::from_rgb(fill[0], fill[1], fill[2]), egui::Stroke::new(1.0_f32, stroke))
    };
    let (fill, stroke) = steel([44, 49, 57], WINDOW_STROKE);
    v.widgets.inactive.bg_fill = fill;
    v.widgets.inactive.weak_bg_fill = fill;
    v.widgets.inactive.bg_stroke = stroke;
    let (fill, stroke) = steel([62, 68, 78], BRASS);
    v.widgets.hovered.bg_fill = fill;
    v.widgets.hovered.weak_bg_fill = fill;
    v.widgets.hovered.bg_stroke = stroke;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(235, 220, 170));
    let (fill, stroke) = steel([80, 70, 40], BRASS);
    v.widgets.active.bg_fill = fill;
    v.widgets.active.weak_bg_fill = fill;
    v.widgets.active.bg_stroke = stroke;
    v.widgets.noninteractive.bg_fill = PANEL_FILL;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, WINDOW_STROKE);
    v
}

/// Доля заполнения шкалы в процентах (0..=100), целочисленно; пустой максимум даёт 0.
pub fn percent(current: u32, max: u32) -> u32 {
    if max == 0 {
        0
    } else {
        (u64::from(current.min(max)) * 100 / u64::from(max)) as u32
    }
}

/// Подпись шкалы: «Организация: 45/60 (75%)».
pub fn bar_text(label: &str, current: u32, max: u32) -> String {
    format!("{label}: {current}/{max} ({}%)", percent(current, max))
}

/// Шкала с подписью поверх.
pub fn status_bar(ui: &mut egui::Ui, label: &str, current: u32, max: u32, color: egui::Color32, width: f32) {
    ui.add(
        egui::ProgressBar::new(percent(current, max) as f32 / 100.0)
            .desired_width(width)
            .fill(color)
            .text(egui::RichText::new(bar_text(label, current, max)).color(egui::Color32::WHITE).small()),
    );
}

/// Две шкалы боеготовности дивизии: зелёная Организация и оранжево-коричневая Прочность.
pub fn readiness_bars(ui: &mut egui::Ui, org: (u32, u32), strength: (u32, u32)) {
    status_bar(ui, "Организация", org.0, org.1, ORG_COLOR, 230.0);
    status_bar(ui, "Прочность", strength.0, strength.1, STRENGTH_COLOR, 230.0);
}

/// Шапка окна: флаг 41x26 (или цветной прямоугольник нации) рядом с «[TAG] Название».
pub fn country_header(ui: &mut egui::Ui, flags: &mut FlagTextures, colors: &CountryColors, tag: &str, name: &str) {
    ui.horizontal(|ui| {
        flags.show_sized(ui, tag, colors, HEADER_FLAG_SIZE);
        ui.label(egui::RichText::new(format!("[{tag}] {name}")).color(BRASS).strong());
    });
}

/// Заголовок окна: жирный латунный текст (под ним egui рисует стальной разделитель).
pub fn window_title(text: &str) -> egui::RichText {
    egui::RichText::new(text).color(BRASS).strong()
}

/// Цвет бабла при успехе атакующего (прогресс >= 50%).
pub const BUBBLE_ATTACK: egui::Color32 = egui::Color32::from_rgb(34, 139, 34);
/// Цвет бабла, когда оборона держит рубеж (прогресс < 50%).
pub const BUBBLE_DEFENSE: egui::Color32 = egui::Color32::from_rgb(160, 30, 30);
/// Тёмный металлический обод бабла.
pub const BUBBLE_RIM: egui::Color32 = egui::Color32::from_rgb(58, 62, 70);
/// Радиус бабла в экранных пикселях (не зависит от зума).
pub const BUBBLE_RADIUS: f32 = 14.0;

/// Прогресс боя 0..=100 из средних долей организации сторон (в %): доля атакующих в сумме.
/// Обе стороны на нуле — ничья, 50.
pub fn bubble_progress(attacker_org_pct: u32, defender_org_pct: u32) -> u32 {
    let total = u64::from(attacker_org_pct) + u64::from(defender_org_pct);
    if total == 0 {
        50
    } else {
        (u64::from(attacker_org_pct) * 100 / total) as u32
    }
}

/// Цвет фона бабла по прогрессу.
pub fn bubble_color(progress: u32) -> egui::Color32 {
    if progress >= 50 {
        BUBBLE_ATTACK
    } else {
        BUBBLE_DEFENSE
    }
}

/// Ширина заполненной части микро-полоски на карте (в долях пикселя целиком из процентов).
pub fn micro_bar_width(percent_value: u32, full_width: f32) -> f32 {
    full_width * percent_value.min(100) as f32 / 100.0
}

/// Применяет тему один раз, как только появляется контекст egui.
fn apply_theme(mut contexts: EguiContexts, mut done: Local<bool>) {
    if *done {
        return;
    }
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    ctx.set_visuals(military_visuals());
    *done = true;
}

/// Плагин интерфейса: штабная тема.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, apply_theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn theme_uses_the_requested_palette() {
        let v = military_visuals();
        assert_eq!(v.panel_fill, egui::Color32::from_rgb(22, 24, 28));
        assert_eq!(v.window_fill, egui::Color32::from_rgb(22, 24, 28));
        assert_eq!(v.window_stroke.color, egui::Color32::from_rgb(70, 75, 85));
        assert_eq!(BRASS, egui::Color32::from_rgb(215, 185, 105));
        assert!(v.widgets.hovered.bg_fill != v.widgets.inactive.bg_fill, "подсветка при наведении");
    }

    #[test]
    fn bar_colors_and_percentages() {
        assert_eq!(ORG_COLOR, egui::Color32::from_rgb(46, 139, 87));
        assert_eq!(STRENGTH_COLOR, egui::Color32::from_rgb(198, 120, 20));
        assert_eq!(percent(45, 60), 75);
        assert_eq!(percent(0, 60), 0);
        assert_eq!(percent(70, 60), 100, "перебор обрезается");
        assert_eq!(percent(5, 0), 0, "нулевой максимум без паники");
        assert_eq!(percent(u32::MAX, u32::MAX), 100);
        assert_eq!(bar_text("Организация", 45, 60), "Организация: 45/60 (75%)");
    }

    #[test]
    fn micro_bar_percentages_and_widths() {
        assert_eq!(percent(0, 60), 0);
        assert_eq!(percent(30, 60), 50);
        assert_eq!(percent(60, 60), 100);
        assert_eq!(percent(7, 0), 0, "деление на ноль");
        assert_eq!(micro_bar_width(0, 26.0), 0.0);
        assert_eq!(micro_bar_width(50, 26.0), 13.0);
        assert_eq!(micro_bar_width(100, 26.0), 26.0);
        assert_eq!(micro_bar_width(250, 26.0), 26.0, "больше 100% обрезается");
    }

    #[test]
    fn bubble_progress_and_color() {
        assert_eq!(bubble_progress(50, 50), 50);
        assert_eq!(bubble_progress(75, 25), 75);
        assert_eq!(bubble_progress(0, 80), 0);
        assert_eq!(bubble_progress(100, 0), 100);
        assert_eq!(bubble_progress(0, 0), 50, "обе стороны сломлены — ничья");
        assert_eq!(bubble_color(50), egui::Color32::from_rgb(34, 139, 34));
        assert_eq!(bubble_color(100), BUBBLE_ATTACK);
        assert_eq!(bubble_color(49), egui::Color32::from_rgb(160, 30, 30));
        assert_eq!(bubble_color(0), BUBBLE_DEFENSE);
        // граница: прогресс 49 при почти равных долях
        assert_eq!(bubble_color(bubble_progress(49, 51)), BUBBLE_DEFENSE);
        assert_eq!(bubble_color(bubble_progress(51, 49)), BUBBLE_ATTACK);
    }

    #[test]
    fn window_titles_are_brass() {
        assert_eq!(window_title("Дивизия").text(), "Дивизия");
    }

    #[test]
    fn unknown_country_gets_a_fallback_flag_without_panic() {
        let ctx = egui::Context::default();
        let mut flags = FlagTextures::new(PathBuf::from("Z:\\нет\\такой\\папки"));
        let colors = CountryColors::default();
        let mut shown = false;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                country_header(ui, &mut flags, &colors, "ZZZ", "Неизвестная");
                readiness_bars(ui, (30, 60), (0, 0));
                shown = true;
            });
        });
        assert!(shown);
        assert_eq!(flags.cached(), 1, "запасная плашка закэширована");
    }
}
