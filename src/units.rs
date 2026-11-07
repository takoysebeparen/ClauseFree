//! Батальоны и шаблоны дивизий: характеристики дивизии считаются из состава батальонов.
//!
//! Все значения целочисленные. Реестр шаблонов (`DivisionTemplates`) сейчас заполняется
//! встроенными определениями; структура рассчитана на последующий парсинг
//! `common/units/*.txt` (типы батальонов) и `division_template` из `history/units/`.
//! Боевая логика читает только рассчитанные `DivisionStats`, а не тип батальона.

use std::collections::BTreeMap;

use bevy::prelude::*;

/// Шаблон, который используется для всех дивизий, чей шаблон ещё не зарегистрирован.
pub const BASE_TEMPLATE: &str = "Infanterie-Division";

/// Скорость пехоты (4.0 км/ч, в десятых долях) — эталон, от которого масштабируется время марша.
pub const INFANTRY_SPEED_KPH: u32 = 40;

/// Тип батальона.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BattalionDef {
    pub manpower: u32,
    pub max_strength: u32,
    pub max_org: u32,
    pub soft_attack: u32,
    pub defense: u32,
    /// Винтовок, необходимых батальону в полном составе.
    pub required_equipment: u32,
    /// Ширина фронта, которую занимает батальон в бою.
    pub combat_width: u32,
    /// Броня: у пехоты 0, у лёгких танков 15, у средних 50.
    pub armor: u32,
    /// Бронепробитие: у базовой пехоты 4, у танков и ПТО 20 и выше.
    pub piercing: u32,
    /// Базовая скорость в км/ч, умноженная на 10 (40 = 4.0 км/ч).
    pub speed_kph: u32,
    /// Твердость (доля бронированной техники), 0..=100 %.
    pub hardness_pct: u32,
}

impl BattalionDef {
    /// Пехотный батальон: 1000 человек, 100 винтовок, 4.0 км/ч, без брони.
    pub fn infantry() -> Self {
        Self {
            manpower: 1000,
            max_strength: 100,
            max_org: 60,
            soft_attack: 10,
            defense: 25,
            required_equipment: 100,
            combat_width: 2,
            armor: 0,
            piercing: 4,
            speed_kph: 40,
            hardness_pct: 0,
        }
    }

    /// Моторизованный батальон: как пехотный, но 12.0 км/ч.
    pub fn motorized() -> Self {
        Self { speed_kph: 120, hardness_pct: 5, ..Self::infantry() }
    }

    /// Лёгкий танковый батальон: броня 15, пробитие 20, 8.0 км/ч.
    pub fn light_tank() -> Self {
        Self {
            manpower: 500,
            max_strength: 100,
            max_org: 40,
            soft_attack: 12,
            defense: 10,
            required_equipment: 80,
            combat_width: 2,
            armor: 15,
            piercing: 20,
            speed_kph: 80,
            hardness_pct: 80,
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
            defense: 15,
            required_equipment: 100,
            combat_width: 2,
            armor: 50,
            piercing: 30,
            speed_kph: 80,
            hardness_pct: 90,
        }
    }
}

/// Итоговые характеристики дивизии, рассчитанные по шаблону.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DivisionStats {
    pub total_manpower: u32,
    pub max_strength: u32,
    pub max_org: u32,
    pub soft_attack: u32,
    pub defense: u32,
    pub total_required_equipment: u32,
    /// Ширина фронта дивизии: сумма по батальонам (пехотная дивизия из 9 батальонов — 18).
    pub combat_width: u32,
    /// Скорость дивизии (км/ч × 10) — по самому медленному батальону.
    pub max_speed: u32,
    /// Броня: `(макс. * 30 + средняя * 70) / 100`.
    pub armor: u32,
    /// Бронепробитие: `(макс. * 40 + среднее * 60) / 100`.
    pub piercing: u32,
    /// Твердость, %: среднее по батальонам.
    pub hardness: u32,
}

#[derive(Debug, Clone)]
pub struct DivisionTemplate {
    pub name: String,
    pub battalions: Vec<BattalionDef>,
}

impl DivisionTemplate {
    /// Сумма по батальонам для всего, кроме организации (среднее с округлением к ближайшему),
    /// скорости (минимум), брони, пробития (взвешенные макс. и среднее) и твёрдости (среднее).
    pub fn calculate_stats(&self) -> DivisionStats {
        let sum = |f: fn(&BattalionDef) -> u32| -> u32 {
            self.battalions.iter().fold(0u32, |acc, b| acc.saturating_add(f(b)))
        };
        let max = |f: fn(&BattalionDef) -> u32| -> u32 { self.battalions.iter().map(f).max().unwrap_or(0) };
        let count = self.battalions.len() as u32;
        let average = |f: fn(&BattalionDef) -> u32| -> u32 { if count == 0 { 0 } else { sum(f) / count } };
        let max_org = if count == 0 { 0 } else { (sum(|b| b.max_org) + count / 2) / count };

        DivisionStats {
            total_manpower: sum(|b| b.manpower),
            max_strength: sum(|b| b.max_strength),
            max_org,
            soft_attack: sum(|b| b.soft_attack),
            defense: sum(|b| b.defense),
            total_required_equipment: sum(|b| b.required_equipment),
            combat_width: sum(|b| b.combat_width),
            max_speed: self.battalions.iter().map(|b| b.speed_kph).min().unwrap_or(0),
            armor: (max(|b| b.armor) * 30 + average(|b| b.armor) * 70) / 100,
            piercing: (max(|b| b.piercing) * 40 + average(|b| b.piercing) * 60) / 100,
            hardness: average(|b| b.hardness_pct),
        }
    }

    pub fn battalion_count(&self) -> u32 {
        self.battalions.len() as u32
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
        let defs = [
            template(BASE_TEMPLATE, &[(BattalionDef::infantry(), 9)]),
            template("Infanterie-Division (mot.)", &[(BattalionDef::motorized(), 9)]),
            template("Panzer-Division", &[(BattalionDef::light_tank(), 4), (BattalionDef::motorized(), 2)]),
        ];
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

    #[test]
    fn infantry_division_stats() {
        let stats = DivisionTemplates::default().get_or_base(BASE_TEMPLATE).calculate_stats();
        assert_eq!(stats.total_manpower, 9000);
        assert_eq!(stats.max_strength, 900);
        assert_eq!(stats.max_org, 60);
        assert_eq!(stats.soft_attack, 90);
        assert_eq!(stats.defense, 225);
        assert_eq!(stats.total_required_equipment, 900);
        assert_eq!(stats.combat_width, 18);
        // пехота: 4.0 км/ч, без брони, пробитие 4, твёрдость 0
        assert_eq!((stats.max_speed, stats.armor, stats.piercing, stats.hardness), (40, 0, 4, 0));
    }

    #[test]
    fn panzer_division_aggregates_armor_piercing_and_speed() {
        // 4 лёгких танка (броня 15, пробитие 20, 8.0 км/ч, твёрдость 80) + 2 моторизованных (12.0 км/ч)
        let stats = DivisionTemplates::default().get_or_base("Panzer-Division").calculate_stats();
        assert_eq!(stats.max_speed, 80, "скорость по самому медленному батальону");
        // броня: макс. 15, средняя 60 / 6 = 10 -> (15 * 30 + 10 * 70) / 100 = 11
        assert_eq!(stats.armor, 11);
        // пробитие: макс. 20, среднее (80 + 8) / 6 = 14 -> (20 * 40 + 14 * 60) / 100 = 16
        assert_eq!(stats.piercing, 16);
        // твёрдость: (4 * 80 + 2 * 5) / 6 = 55
        assert_eq!(stats.hardness, 55);
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
    fn medium_tank_armor_uses_weighted_rule() {
        // 3 средних танка + 3 пехотных: макс. 50, средняя 25 -> (50 * 30 + 25 * 70) / 100 = 32
        let stats = template("m", &[(BattalionDef::medium_tank(), 3), (BattalionDef::infantry(), 3)]).calculate_stats();
        assert_eq!(stats.armor, 32);
    }
}
