//! Батальоны и шаблоны дивизий: характеристики дивизии считаются из состава батальонов.
//!
//! Все значения целочисленные. Реестр шаблонов (`DivisionTemplates`) сейчас заполняется
//! встроенными определениями; структура рассчитана на последующий парсинг
//! `common/units/*.txt` (типы батальонов) и `division_template` из `history/units/`.
//! Боевая логика читает только рассчитанные `DivisionStats`, а не тип батальона.
//!
//! Роты обеспечения (`is_support`) — артиллерия, сапёры, разведка — не занимают ширину фронта
//! и не участвуют в средних значениях (организация, броня, пробитие, твёрдость), но добавляют
//! свои эффекты: атаку, окопанность, форсирование рек, разведку.

use std::collections::BTreeMap;

use bevy::prelude::*;

/// Шаблон, который используется для всех дивизий, чей шаблон ещё не зарегистрирован.
pub const BASE_TEMPLATE: &str = "Infanterie-Division";

/// Шаблон из 9 пехотных батальонов без рот обеспечения: стабильная точка отсчёта для тестов формул.
#[cfg(test)]
pub const PLAIN_INFANTRY_TEMPLATE: &str = "Infantry-Plain";

/// Скорость пехоты (4.0 км/ч, в десятых долях) — эталон, от которого масштабируется время марша.
pub const INFANTRY_SPEED_KPH: u32 = 40;

/// Максимальная окопанность дивизии без сапёров.
pub const BASE_MAX_ENTRENCHMENT: u32 = 10;

/// Подписи рот обеспечения в интерфейсе (в таком порядке они и выводятся).
pub const SUPPORT_ENGINEERS: &str = "Сапёры";
pub const SUPPORT_ARTILLERY: &str = "Арт";
pub const SUPPORT_RECON: &str = "Разведка";
const SUPPORT_LABELS: [&str; 3] = [SUPPORT_ENGINEERS, SUPPORT_ARTILLERY, SUPPORT_RECON];

/// Тип батальона или роты обеспечения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BattalionDef {
    pub manpower: u32,
    pub max_strength: u32,
    pub max_org: u32,
    pub soft_attack: u32,
    /// Жёсткая атака (против техники, твёрдость цели высокая). У пехоты минимальна (0), у танков и артиллерии выше.
    /// Вместе с soft_attack даёт силу удара по формуле (soft * (100 - T) + hard * T) / 100, T — твёрдость цели.
    pub hard_attack: u32,
    pub defense: u32,
    /// Винтовок, необходимых батальону в полном составе.
    pub required_equipment: u32,
    /// Ширина фронта, которую занимает батальон в бою (у рот обеспечения 0).
    pub combat_width: u32,
    /// Броня: у пехоты 0, у лёгких танков 15, у средних 50.
    pub armor: u32,
    /// Бронепробитие: у базовой пехоты 4, у танков и ПТО 20 и выше.
    pub piercing: u32,
    /// Базовая скорость в км/ч, умноженная на 10 (40 = 4.0 км/ч).
    pub speed_kph: u32,
    /// Твердость (доля бронированной техники), 0..=100 %.
    pub hardness_pct: u32,
    /// Разведка: пехота 1, моторизованные 3, лёгкие танки 2, средние 1.
    pub recon: u32,
    /// Рота обеспечения: не занимает ширину фронта и не входит в средние значения.
    pub is_support: bool,
    /// Прибавка к максимальной окопанности дивизии (сапёры: +5, то есть 10 -> 15).
    pub entrenchment_bonus: u32,
    /// На сколько пунктов снижается штраф форсирования реки (сапёры: 20, то есть -30% -> -10%).
    pub river_penalty_reduction: u32,
    /// Подпись роты обеспечения для интерфейса; у обычных батальонов пустая.
    pub support_label: &'static str,
}

impl BattalionDef {
    /// Пехотный батальон: 1000 человек, 100 винтовок, 4.0 км/ч, без брони.
    pub fn infantry() -> Self {
        Self {
            manpower: 1000,
            max_strength: 100,
            max_org: 60,
            soft_attack: 10,
            hard_attack: 0,
            defense: 25,
            required_equipment: 100,
            combat_width: 2,
            armor: 0,
            piercing: 4,
            speed_kph: 40,
            hardness_pct: 0,
            recon: 1,
            is_support: false,
            entrenchment_bonus: 0,
            river_penalty_reduction: 0,
            support_label: "",
        }
    }

    /// Моторизованный батальон: как пехотный, но 12.0 км/ч.
    pub fn motorized() -> Self {
        Self { speed_kph: 120, hardness_pct: 5, recon: 3, ..Self::infantry() }
    }

    /// Лёгкий танковый батальон: броня 15, пробитие 20, 8.0 км/ч.
    pub fn light_tank() -> Self {
        Self {
            manpower: 500,
            max_strength: 100,
            max_org: 40,
            soft_attack: 12,
            hard_attack: 10,
            defense: 10,
            required_equipment: 80,
            combat_width: 2,
            armor: 15,
            piercing: 20,
            speed_kph: 80,
            hardness_pct: 80,
            recon: 2,
            ..Self::infantry()
        }
    }

    /// Средний танковый батальон: броня 50, пробитие 30, 8.0 км/ч.
    /// Пока используется только в тестах: в OOB 1936 нет шаблонов со средними танками.
    #[allow(dead_code)]
    pub fn medium_tank() -> Self {
        Self {
            manpower: 500,
            max_strength: 100,
            max_org: 40,
            soft_attack: 20,
            hard_attack: 24,
            defense: 15,
            required_equipment: 100,
            combat_width: 2,
            armor: 50,
            piercing: 30,
            speed_kph: 80,
            hardness_pct: 90,
            recon: 1,
            ..Self::infantry()
        }
    }

    /// Основа роты обеспечения: без прочности, организации, ширины и винтовок (у неё свои виды снаряжения).
    fn support() -> Self {
        Self {
            manpower: 0,
            max_strength: 0,
            max_org: 0,
            soft_attack: 0,
            hard_attack: 0,
            defense: 0,
            required_equipment: 0,
            combat_width: 0,
            armor: 0,
            piercing: 0,
            speed_kph: INFANTRY_SPEED_KPH,
            hardness_pct: 0,
            recon: 0,
            is_support: true,
            entrenchment_bonus: 0,
            river_penalty_reduction: 0,
            support_label: "",
        }
    }

    /// Артиллерия поддержки: прямая прибавка к атаке дивизии, ширину фронта не занимает.
    pub fn support_artillery() -> Self {
        Self {
            manpower: 500,
            soft_attack: 25,
            hard_attack: 2,
            piercing: 5,
            support_label: SUPPORT_ARTILLERY,
            ..Self::support()
        }
    }

    /// Сапёры: окопанность до 15 и штраф форсирования реки -10% вместо -30%.
    pub fn engineers() -> Self {
        Self {
            manpower: 300,
            entrenchment_bonus: 5,
            river_penalty_reduction: 20,
            support_label: SUPPORT_ENGINEERS,
            ..Self::support()
        }
    }

    /// Рота разведки: разведка 2.
    #[allow(dead_code)]
    pub fn support_recon() -> Self {
        Self { manpower: 200, recon: 2, support_label: SUPPORT_RECON, ..Self::support() }
    }
}

/// Итоговые характеристики дивизии, рассчитанные по шаблону.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DivisionStats {
    pub total_manpower: u32,
    pub max_strength: u32,
    pub max_org: u32,
    pub soft_attack: u32,
    /// Жёсткая атака: сумма по батальонам и ротам обеспечения (мягкая и жёсткая хранятся раздельно).
    pub hard_attack: u32,
    pub defense: u32,
    pub total_required_equipment: u32,
    /// Ширина фронта дивизии: сумма по батальонам (пехотная дивизия из 9 батальонов — 18);
    /// роты обеспечения ширину не занимают.
    pub combat_width: u32,
    /// Скорость дивизии (км/ч × 10) — по самому медленному батальону.
    pub max_speed: u32,
    /// Броня: `(макс. * 30 + средняя * 70) / 100`.
    pub armor: u32,
    /// Бронепробитие: `(макс. * 40 + среднее * 60) / 100`.
    pub piercing: u32,
    /// Твердость, %: среднее по батальонам.
    pub hardness: u32,
    /// Разведка дивизии: максимум среди батальонов и рот обеспечения.
    pub recon: u32,
    /// В дивизии есть сапёры.
    pub has_engineers: bool,
    /// Максимальная окопанность: 10, с сапёрами 15.
    pub max_entrenchment: u32,
    /// На сколько пунктов снижен штраф форсирования реки (с сапёрами 20).
    pub river_penalty_reduction: u32,
    /// Роты обеспечения для интерфейса в порядке «Сапёры, Арт, Разведка».
    pub support: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub struct DivisionTemplate {
    pub name: String,
    pub battalions: Vec<BattalionDef>,
}

impl DivisionTemplate {
    /// Сумма по батальонам для всего, кроме организации (среднее с округлением к ближайшему),
    /// скорости (минимум), брони, пробития (взвешенные макс. и среднее) и твёрдости (среднее).
    /// Средние считаются по боевым батальонам: роты обеспечения их не разбавляют, зато участвуют
    /// в суммах (атака, личный состав), в максимумах (броня, пробитие, разведка) и в минимуме скорости.
    pub fn calculate_stats(&self) -> DivisionStats {
        let all = &self.battalions;
        let combat: Vec<&BattalionDef> = all.iter().filter(|b| !b.is_support).collect();
        let combat_count = combat.len() as u32;

        let sum = |f: fn(&BattalionDef) -> u32| -> u32 { all.iter().fold(0u32, |acc, b| acc.saturating_add(f(b))) };
        let max = |f: fn(&BattalionDef) -> u32| -> u32 { all.iter().map(f).max().unwrap_or(0) };
        let average = |f: fn(&BattalionDef) -> u32| -> u32 {
            if combat_count == 0 { 0 } else { combat.iter().map(|b| f(b)).sum::<u32>() / combat_count }
        };
        let max_org = if combat_count == 0 {
            0
        } else {
            (combat.iter().map(|b| b.max_org).sum::<u32>() + combat_count / 2) / combat_count
        };
        let has_engineers = all.iter().any(|b| b.is_support && b.entrenchment_bonus > 0);

        DivisionStats {
            total_manpower: sum(|b| b.manpower),
            max_strength: sum(|b| b.max_strength),
            max_org,
            soft_attack: sum(|b| b.soft_attack),
            hard_attack: sum(|b| b.hard_attack),
            defense: sum(|b| b.defense),
            total_required_equipment: sum(|b| b.required_equipment),
            combat_width: all.iter().filter(|b| !b.is_support).map(|b| b.combat_width).sum(),
            max_speed: all.iter().map(|b| b.speed_kph).min().unwrap_or(0),
            armor: (max(|b| b.armor) * 30 + average(|b| b.armor) * 70) / 100,
            piercing: (max(|b| b.piercing) * 40 + average(|b| b.piercing) * 60) / 100,
            hardness: average(|b| b.hardness_pct),
            recon: max(|b| b.recon),
            has_engineers,
            max_entrenchment: BASE_MAX_ENTRENCHMENT + max(|b| b.entrenchment_bonus),
            river_penalty_reduction: max(|b| b.river_penalty_reduction),
            support: SUPPORT_LABELS.into_iter().filter(|l| all.iter().any(|b| b.support_label == *l)).collect(),
        }
    }

    /// Число боевых батальонов (роты обеспечения не считаются).
    pub fn battalion_count(&self) -> u32 {
        self.battalions.iter().filter(|b| !b.is_support).count() as u32
    }
}

/// Шаблон из списка «(тип батальона, сколько)».
fn template(name: &str, composition: &[(BattalionDef, usize)]) -> DivisionTemplate {
    let battalions = composition.iter().flat_map(|(def, n)| std::iter::repeat(def.clone()).take(*n)).collect();
    DivisionTemplate { name: name.to_string(), battalions }
}

/// Реестр шаблонов по имени. `BTreeMap` — детерминированный порядок.
#[derive(Resource)]
pub struct DivisionTemplates {
    templates: BTreeMap<String, DivisionTemplate>,
}

impl Default for DivisionTemplates {
    fn default() -> Self {
        #[allow(unused_mut)] // в тестовой сборке в реестр добавляется шаблон без поддержки
        let mut defs = vec![
            // стандартная пехотная дивизия: 9 батальонов + сапёры + артиллерия поддержки
            template(
                BASE_TEMPLATE,
                &[(BattalionDef::infantry(), 9), (BattalionDef::engineers(), 1), (BattalionDef::support_artillery(), 1)],
            ),
            template("Infanterie-Division (mot.)", &[(BattalionDef::motorized(), 9)]),
            template("Panzer-Division", &[(BattalionDef::light_tank(), 4), (BattalionDef::motorized(), 2)]),
        ];
        #[cfg(test)]
        defs.push(template(PLAIN_INFANTRY_TEMPLATE, &[(BattalionDef::infantry(), 9)]));
        Self { templates: defs.into_iter().map(|t| (t.name.clone(), t)).collect() }
    }
}

impl DivisionTemplates {
    /// Шаблон по имени или базовый, если такого нет в реестре.
    pub fn get_or_base(&self, name: &str) -> &DivisionTemplate {
        self.templates
            .get(name)
            .or_else(|| self.templates.get(BASE_TEMPLATE))
            .expect("базовый шаблон всегда зарегистрирован")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats_of(name: &str) -> DivisionStats {
        DivisionTemplates::default().get_or_base(name).calculate_stats()
    }

    #[test]
    fn plain_infantry_division_stats() {
        let stats = stats_of(PLAIN_INFANTRY_TEMPLATE);
        assert_eq!(stats.total_manpower, 9000);
        assert_eq!(stats.max_strength, 900);
        assert_eq!(stats.max_org, 60);
        assert_eq!(stats.soft_attack, 90);
        assert_eq!(stats.defense, 225);
        assert_eq!(stats.total_required_equipment, 900);
        assert_eq!(stats.combat_width, 18);
        // пехота: 4.0 км/ч, без брони, пробитие 4, твёрдость 0
        assert_eq!((stats.max_speed, stats.armor, stats.piercing, stats.hardness), (40, 0, 4, 0));
        assert_eq!(stats.recon, 1);
        // без поддержки: окопанность до 10, реки в полную силу
        assert_eq!((stats.has_engineers, stats.max_entrenchment, stats.river_penalty_reduction), (false, 10, 0));
        assert!(stats.support.is_empty());
    }

    #[test]
    fn standard_infantry_division_has_engineers_and_artillery_without_extra_width() {
        let base = stats_of(BASE_TEMPLATE);
        let plain = stats_of(PLAIN_INFANTRY_TEMPLATE);
        // состав: 9 пехотных + сапёры + артиллерия; шаблон по-прежнему «9 батальонов»
        assert_eq!(DivisionTemplates::default().get_or_base(BASE_TEMPLATE).battalion_count(), 9);
        // поддержка не занимает ширину фронта и не меняет прочность, организацию, защиту, снаряжение
        assert_eq!(base.combat_width, 18);
        assert_eq!((base.max_strength, base.max_org, base.defense, base.total_required_equipment), (900, 60, 225, 900));
        // артиллерия: +25 мягкой и +2 жёсткой атаки; личный состав роты: 500 + 300
        assert_eq!(base.soft_attack, plain.soft_attack + 25);
        assert_eq!(base.hard_attack, 2);
        assert_eq!(base.total_manpower, 9000 + 500 + 300);
        // пробитие: макс. 5 (артиллерия), среднее по боевым 4 -> (5 * 40 + 4 * 60) / 100 = 4 — пехота не бронебойнее
        assert_eq!((base.piercing, base.armor, base.hardness, base.max_speed, base.recon), (4, 0, 0, 40, 1));
        // сапёры
        assert!(base.has_engineers);
        assert_eq!(base.max_entrenchment, 15);
        assert_eq!(base.river_penalty_reduction, 20);
        assert_eq!(base.support, vec!["Сапёры", "Арт"]);
    }

    #[test]
    fn support_companies_are_zero_width_and_do_not_dilute_averages() {
        let support = [BattalionDef::support_artillery(), BattalionDef::engineers(), BattalionDef::support_recon()];
        for company in &support {
            assert!(company.is_support);
            assert_eq!(company.combat_width, 0, "{} не занимает ширину", company.support_label);
        }
        // артиллерия: 25 / 2 / броня 0 / пробитие 5 / 4.0 км/ч
        let art = BattalionDef::support_artillery();
        assert_eq!((art.soft_attack, art.hard_attack, art.armor, art.piercing, art.speed_kph), (25, 2, 0, 5, 40));
        // сапёры: +5 окопанности, -20 к штрафу реки
        let eng = BattalionDef::engineers();
        assert_eq!((eng.entrenchment_bonus, eng.river_penalty_reduction), (5, 20));
        // разведка: 2
        assert_eq!(BattalionDef::support_recon().recon, 2);

        // даже десяток рот не меняет ширину и среднюю организацию
        let many = template("many", &[(BattalionDef::infantry(), 3), (BattalionDef::engineers(), 5), (BattalionDef::support_recon(), 5)]);
        let stats = many.calculate_stats();
        assert_eq!(stats.combat_width, 6);
        assert_eq!(stats.max_org, 60);
        assert_eq!(stats.hardness, 0);
        assert_eq!(many.battalion_count(), 3);
        // несколько сапёров не складывают бонус: окопанность не выше 15
        assert_eq!(stats.max_entrenchment, 15);
        // разведка поддержки поднимает разведку всей дивизии
        assert_eq!(stats.recon, 2);
        assert_eq!(stats.support, vec!["Сапёры", "Разведка"]);
    }

    #[test]
    fn division_without_engineers_keeps_base_limits() {
        let stats = template("arty only", &[(BattalionDef::infantry(), 9), (BattalionDef::support_artillery(), 1)]).calculate_stats();
        assert!(!stats.has_engineers);
        assert_eq!((stats.max_entrenchment, stats.river_penalty_reduction), (10, 0));
        assert_eq!(stats.support, vec!["Арт"]);
    }

    #[test]
    fn panzer_division_aggregates_armor_piercing_and_speed() {
        // 4 лёгких танка (броня 15, пробитие 20, 8.0 км/ч, твёрдость 80) + 2 моторизованных (12.0 км/ч)
        let stats = stats_of("Panzer-Division");
        assert_eq!(stats.max_speed, 80, "скорость по самому медленному батальону");
        // броня: макс. 15, средняя 60 / 6 = 10 -> (15 * 30 + 10 * 70) / 100 = 11
        assert_eq!(stats.armor, 11);
        // пробитие: макс. 20, среднее (80 + 8) / 6 = 14 -> (20 * 40 + 14 * 60) / 100 = 16
        assert_eq!(stats.piercing, 16);
        // твёрдость: (4 * 80 + 2 * 5) / 6 = 55
        assert_eq!(stats.hardness, 55);
        // разведка: максимум среди батальонов — моторизованные (3) лучше лёгких танков (2)
        assert_eq!(stats.recon, 3);
        // у танковой дивизии нет поддержки
        assert!(stats.support.is_empty() && !stats.has_engineers);
    }

    #[test]
    fn slowest_battalion_sets_division_speed() {
        // Моторизованная дивизия — 12.0 км/ч, а одна пехотная рота в её составе тянет всю дивизию до 4.0
        let motorized = template("mot", &[(BattalionDef::motorized(), 9)]).calculate_stats();
        assert_eq!(motorized.max_speed, 120);
        let mixed = template("mixed", &[(BattalionDef::motorized(), 8), (BattalionDef::infantry(), 1)]).calculate_stats();
        assert_eq!(mixed.max_speed, 40);
    }

    #[test]
    fn battalion_recon_values_and_division_takes_the_maximum() {
        assert_eq!(BattalionDef::infantry().recon, 1);
        assert_eq!(BattalionDef::motorized().recon, 3);
        assert_eq!(BattalionDef::light_tank().recon, 2);
        assert_eq!(BattalionDef::medium_tank().recon, 1);
        // одна моторизованная рота поднимает разведку всей дивизии, остальные батальоны не снижают
        let mixed = template("mixed", &[(BattalionDef::infantry(), 8), (BattalionDef::motorized(), 1)]).calculate_stats();
        assert_eq!(mixed.recon, 3);
        assert_eq!(template("tanks", &[(BattalionDef::light_tank(), 4)]).calculate_stats().recon, 2);
        assert_eq!(template("medium", &[(BattalionDef::medium_tank(), 3)]).calculate_stats().recon, 1);
        assert_eq!(template("empty", &[]).calculate_stats().recon, 0);
    }

    #[test]
    fn medium_tank_armor_uses_weighted_rule() {
        // 3 средних танка + 3 пехотных: макс. 50, средняя 25 -> (50 * 30 + 25 * 70) / 100 = 32
        let stats = template("m", &[(BattalionDef::medium_tank(), 3), (BattalionDef::infantry(), 3)]).calculate_stats();
        assert_eq!(stats.armor, 32);
    }
}
