//! Батальоны и шаблоны дивизий: характеристики дивизии считаются из состава батальонов.
//!
//! Все значения целочисленные. Реестр шаблонов (`DivisionTemplates`) сейчас заполняется
//! встроенными определениями; структура рассчитана на последующий парсинг
//! `common/units/*.txt` (типы батальонов) и `division_template` из `history/units/`.

use std::collections::BTreeMap;

use bevy::prelude::*;

/// Шаблон, который используется для всех дивизий, чей шаблон ещё не зарегистрирован.
pub const BASE_TEMPLATE: &str = "Infanterie-Division";

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
}

impl BattalionDef {
    /// Пехотный батальон: 1000 человек, 100 винтовок.
    pub fn infantry() -> Self {
        Self {
            manpower: 1000,
            max_strength: 100,
            max_org: 60,
            soft_attack: 10,
            defense: 25,
            required_equipment: 100,
            combat_width: 2,
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
}

#[derive(Debug, Clone)]
pub struct DivisionTemplate {
    pub name: String,
    pub battalions: Vec<BattalionDef>,
}

impl DivisionTemplate {
    /// Сумма по батальонам для всего, кроме организации; организация — среднее арифметическое
    /// (с округлением к ближайшему целому), как в HoI4.
    pub fn calculate_stats(&self) -> DivisionStats {
        let sum = |f: fn(&BattalionDef) -> u32| -> u32 {
            self.battalions.iter().fold(0u32, |acc, b| acc.saturating_add(f(b)))
        };
        let count = self.battalions.len() as u32;
        let org_sum = sum(|b| b.max_org);
        let max_org = if count == 0 { 0 } else { (org_sum + count / 2) / count };

        DivisionStats {
            total_manpower: sum(|b| b.manpower),
            max_strength: sum(|b| b.max_strength),
            max_org,
            soft_attack: sum(|b| b.soft_attack),
            defense: sum(|b| b.defense),
            total_required_equipment: sum(|b| b.required_equipment),
            combat_width: sum(|b| b.combat_width),
        }
    }

    pub fn battalion_count(&self) -> u32 {
        self.battalions.len() as u32
    }
}

/// Реестр шаблонов по имени. `BTreeMap` — детерминированный порядок.
#[derive(Resource)]
pub struct DivisionTemplates {
    templates: BTreeMap<String, DivisionTemplate>,
}

impl Default for DivisionTemplates {
    fn default() -> Self {
        let mut templates = BTreeMap::new();
        templates.insert(
            BASE_TEMPLATE.to_string(),
            DivisionTemplate { name: BASE_TEMPLATE.to_string(), battalions: vec![BattalionDef::infantry(); 9] },
        );
        Self { templates }
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
    }
}
