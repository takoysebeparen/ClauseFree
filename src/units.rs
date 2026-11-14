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

use crate::economy::EquipmentType;

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

/// Потолок опыта дивизии, %.
pub const MAX_EXPERIENCE_PCT: u32 = 100;

/// Ранг опыта дивизии по канону HoI4: определяет множитель боевой мощи.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DivisionExperienceLevel {
    /// 0..=9%: -25%.
    Green,
    /// 10..=29%: без изменений.
    Trained,
    /// 30..=59%: +25%.
    Regular,
    /// 60..=89%: +50%.
    Seasoned,
    /// 90..=100%: +75%.
    Veteran,
}

impl DivisionExperienceLevel {
    /// Ранг по проценту опыта (значения выше 100 приравниваются к 100).
    pub fn from_experience_pct(pct: u32) -> DivisionExperienceLevel {
        match pct.min(MAX_EXPERIENCE_PCT) {
            0..=9 => DivisionExperienceLevel::Green,
            10..=29 => DivisionExperienceLevel::Trained,
            30..=59 => DivisionExperienceLevel::Regular,
            60..=89 => DivisionExperienceLevel::Seasoned,
            _ => DivisionExperienceLevel::Veteran,
        }
    }

    /// Множитель боевой мощи в процентах: 75, 100, 125, 150, 175.
    pub fn combat_multiplier_pct(&self) -> i32 {
        match self {
            DivisionExperienceLevel::Green => 75,
            DivisionExperienceLevel::Trained => 100,
            DivisionExperienceLevel::Regular => 125,
            DivisionExperienceLevel::Seasoned => 150,
            DivisionExperienceLevel::Veteran => 175,
        }
    }

    /// Название ранга для интерфейса.
    pub fn title(&self) -> &'static str {
        match self {
            DivisionExperienceLevel::Green => "Новобранцы",
            DivisionExperienceLevel::Trained => "Обученные",
            DivisionExperienceLevel::Regular => "Кадровые",
            DivisionExperienceLevel::Seasoned => "Опытные",
            DivisionExperienceLevel::Veteran => "Ветераны",
        }
    }
}

/// Тактический символ NATO дивизии на фишке карты.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NatoSymbol {
    /// Пехота: косой крест X.
    #[default]
    Infantry,
    /// Кавалерия: одна диагональная черта.
    Cavalry,
    /// Моторизованная пехота: крест с колёсами снизу.
    Motorized,
    /// Бронетехника: горизонтальный эллипс.
    Armored,
    /// Артиллерия: точка по центру.
    Artillery,
}

impl NatoSymbol {
    /// Название типа для интерфейса: `[NATO: Пехота]`.
    pub fn label(self) -> &'static str {
        match self {
            NatoSymbol::Infantry => "Пехота",
            NatoSymbol::Cavalry => "Кавалерия",
            NatoSymbol::Motorized => "Мотопехота",
            NatoSymbol::Armored => "Танки",
            NatoSymbol::Artillery => "Артиллерия",
        }
    }
}

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
    /// Тип и количество снаряжения, необходимого батальону в полном составе
    /// (пехота — 100 пехотного, артиллерия — 24 орудия, лёгкий танк — 50, сапёры — 10 оснащения).
    pub equipment_needs: (EquipmentType, u32),
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
    /// Символ батальона по его данным: танковое снаряжение -> бронетехника, орудия -> артиллерия,
    /// скорость от 10.0 км/ч -> моторизованный, от 6.0 км/ч -> конный, иначе пехота.
    pub fn nato_symbol(&self) -> NatoSymbol {
        match self.equipment_needs.0 {
            EquipmentType::LightTank | EquipmentType::MediumTank => NatoSymbol::Armored,
            EquipmentType::Artillery => NatoSymbol::Artillery,
            _ if self.speed_kph >= 100 => NatoSymbol::Motorized,
            _ if self.speed_kph >= 60 => NatoSymbol::Cavalry,
            _ => NatoSymbol::Infantry,
        }
    }

    /// Пехотный батальон: 1000 человек, 100 винтовок, 4.0 км/ч, без брони.
    pub fn infantry() -> Self {
        Self {
            manpower: 1000,
            max_strength: 100,
            max_org: 60,
            soft_attack: 10,
            hard_attack: 0,
            defense: 25,
            equipment_needs: (EquipmentType::InfantryEquipment, 100),
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
            equipment_needs: (EquipmentType::LightTank, 50),
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
            equipment_needs: (EquipmentType::MediumTank, 50),
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
            equipment_needs: (EquipmentType::InfantryEquipment, 0),
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
            equipment_needs: (EquipmentType::Artillery, 24),
            support_label: SUPPORT_ARTILLERY,
            ..Self::support()
        }
    }

    /// Сапёры: окопанность до 15 и штраф форсирования реки -10% вместо -30%.
    pub fn engineers() -> Self {
        Self {
            manpower: 300,
            equipment_needs: (EquipmentType::SupportEquipment, 10),
            entrenchment_bonus: 5,
            river_penalty_reduction: 20,
            support_label: SUPPORT_ENGINEERS,
            ..Self::support()
        }
    }

    /// Конный батальон: 600 человек, 6.0 км/ч, разведка 2.
    pub fn cavalry() -> Self {
        Self { manpower: 600, max_org: 40, soft_attack: 8, defense: 15, speed_kph: 60, recon: 2, ..Self::infantry() }
    }

    /// Горные стрелки: пехота с повышенной организацией и защитой.
    pub fn mountaineers() -> Self {
        Self { max_org: 65, defense: 30, ..Self::infantry() }
    }

    /// Моторизованная рота разведки (бронеавтомобили и мотоциклы): разведка 3.
    pub fn support_mot_recon() -> Self {
        Self { recon: 3, ..Self::support_recon() }
    }

    /// Рота разведки: разведка 2.
    pub fn support_recon() -> Self {
        Self {
            manpower: 200,
            recon: 2,
            equipment_needs: (EquipmentType::SupportEquipment, 5),
            support_label: SUPPORT_RECON,
            ..Self::support()
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
    /// Жёсткая атака: сумма по батальонам и ротам обеспечения (мягкая и жёсткая хранятся раздельно).
    pub hard_attack: u32,
    pub defense: u32,
    /// Суммарная потребность дивизии в снаряжении по типам для полного укомплектования
    /// (нулевые потребности в карту не попадают).
    pub equipment_needs: BTreeMap<EquipmentType, u32>,
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
    /// Доминирующий символ NATO шаблона.
    pub nato_symbol: NatoSymbol,
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
            equipment_needs: {
                let mut needs = BTreeMap::new();
                for b in all.iter().filter(|b| b.equipment_needs.1 > 0) {
                    *needs.entry(b.equipment_needs.0).or_insert(0u32) += b.equipment_needs.1;
                }
                needs
            },
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
            nato_symbol: self.nato_symbol(),
        }
    }

    /// Доминирующий символ по боевым батальонам (роты обеспечения не считаются): есть танки -> бронетехника;
    /// артиллерия или моторизованные — от половины батальонов; конные — от половины; иначе пехота.
    pub fn nato_symbol(&self) -> NatoSymbol {
        let combat: Vec<NatoSymbol> = self.battalions.iter().filter(|b| !b.is_support).map(BattalionDef::nato_symbol).collect();
        let total = combat.len();
        let count = |symbol: NatoSymbol| combat.iter().filter(|&&s| s == symbol).count();
        if count(NatoSymbol::Armored) > 0 {
            NatoSymbol::Armored
        } else if total > 0 && count(NatoSymbol::Artillery) * 2 > total {
            NatoSymbol::Artillery
        } else if total > 0 && count(NatoSymbol::Motorized) * 2 >= total {
            NatoSymbol::Motorized
        } else if total > 0 && count(NatoSymbol::Cavalry) * 2 >= total {
            NatoSymbol::Cavalry
        } else {
            NatoSymbol::Infantry
        }
    }

    /// Число боевых батальонов (роты обеспечения не считаются).
    pub fn battalion_count(&self) -> u32 {
        self.battalions.iter().filter(|b| !b.is_support).count() as u32
    }
}

/// Названия французских шаблонов из `history/units/FRA_1936.txt`.
pub const FRA_TEMPLATE_NAMES: [&str; 8] = [
    "Division d'Infanterie",
    "Division Coloniale",
    "Division d'Infanterie Motorisée",
    "Division d'Infanterie Alpine",
    "Division de Cavalerie",
    "Division Légère Mécanique",
    "Brigade de Chars de Combat",
    "Brigade Coloniale",
];

/// Шаблон дивизии из скрипта: имя, типы полков и рот поддержки по порядку файла.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TemplateSpec {
    pub name: String,
    pub regiments: Vec<String>,
    pub support: Vec<String>,
}

/// Читает все `division_template = { name = ".." regiments = { тип = { x y } .. } support = { .. } }`.
/// Мусор и обрезанный текст дают меньше шаблонов, но не панику.
pub fn parse_division_templates(text: &str) -> Vec<TemplateSpec> {
    let tokens = crate::map::tokenize(text);
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut current: Option<TemplateSpec> = None;
    for i in 0..tokens.len() {
        match tokens[i].as_str() {
            "{" => {
                let key = if i >= 2 && tokens[i - 1] == "=" { tokens[i - 2].clone() } else { String::new() };
                stack.push(key);
                if stack.len() == 1 && stack[0] == "division_template" {
                    current = Some(TemplateSpec::default());
                }
            }
            "}" => {
                if stack.len() == 1 && stack[0] == "division_template" {
                    if let Some(spec) = current.take().filter(|s| !s.name.is_empty()) {
                        out.push(spec);
                    }
                }
                stack.pop();
            }
            "=" => {}
            key => {
                if tokens.get(i + 1).map(String::as_str) != Some("=") {
                    continue;
                }
                let (Some(value), Some(spec)) = (tokens.get(i + 2), current.as_mut()) else { continue };
                match stack.as_slice() {
                    [t] if t == "division_template" && key == "name" => spec.name = value.clone(),
                    [t, r] if t == "division_template" && r == "regiments" && value == "{" => spec.regiments.push(key.to_string()),
                    [t, r] if t == "division_template" && r == "support" && value == "{" => spec.support.push(key.to_string()),
                    _ => {}
                }
            }
        }
    }
    out
}

/// Собирает шаблон по скрипту. Неизвестный тип полка или роты -> `None` (шаблон остаётся прежним).
/// Роты поддержки едут со скоростью самого медленного боевого батальона, чтобы не тормозить колонну.
pub fn build_template(spec: &TemplateSpec) -> Option<DivisionTemplate> {
    let mut battalions = Vec::new();
    for kind in &spec.regiments {
        battalions.push(match kind.as_str() {
            "infantry" => BattalionDef::infantry(),
            "motorized" => BattalionDef::motorized(),
            "mountaineers" => BattalionDef::mountaineers(),
            "cavalry" => BattalionDef::cavalry(),
            "light_armor" => BattalionDef::light_tank(),
            _ => return None,
        });
    }
    if battalions.is_empty() {
        return None;
    }
    let speed = battalions.iter().map(|b| b.speed_kph).min().unwrap_or(INFANTRY_SPEED_KPH);
    for kind in &spec.support {
        let mut def = match kind.as_str() {
            "artillery" => BattalionDef::support_artillery(),
            "engineer" => BattalionDef::engineers(),
            "recon" => BattalionDef::support_recon(),
            "mot_recon" => BattalionDef::support_mot_recon(),
            _ => return None,
        };
        def.speed_kph = speed;
        battalions.push(def);
    }
    Some(DivisionTemplate { name: spec.name.clone(), battalions })
}

/// Встроенные копии французских шаблонов (состав как в `FRA_1936.txt`) — запас, если игры нет.
fn fra_fallback_specs() -> Vec<TemplateSpec> {
    let spec = |name: &str, regiments: &[(&str, usize)], support: &[&str]| TemplateSpec {
        name: name.to_string(),
        regiments: regiments.iter().flat_map(|&(k, n)| std::iter::repeat(k.to_string()).take(n)).collect(),
        support: support.iter().map(|k| k.to_string()).collect(),
    };
    vec![
        spec(FRA_TEMPLATE_NAMES[0], &[("infantry", 9)], &["artillery", "engineer"]),
        spec(FRA_TEMPLATE_NAMES[1], &[("infantry", 9)], &["artillery"]),
        spec(FRA_TEMPLATE_NAMES[2], &[("motorized", 9)], &["mot_recon", "artillery", "engineer"]),
        spec(FRA_TEMPLATE_NAMES[3], &[("mountaineers", 12)], &[]),
        spec(FRA_TEMPLATE_NAMES[4], &[("cavalry", 4)], &["recon"]),
        spec(FRA_TEMPLATE_NAMES[5], &[("light_armor", 2), ("motorized", 4)], &["mot_recon", "engineer"]),
        spec(FRA_TEMPLATE_NAMES[6], &[("light_armor", 4)], &[]),
        spec(FRA_TEMPLATE_NAMES[7], &[("infantry", 6)], &[]),
    ]
}

/// Шаблон из списка «(тип батальона, сколько)».
fn template(name: &str, composition: &[(BattalionDef, usize)]) -> DivisionTemplate {
    let battalions = composition.iter().flat_map(|(def, n)| std::iter::repeat(def.clone()).take(*n)).collect();
    DivisionTemplate { name: name.to_string(), battalions }
}

impl DivisionStats {
    /// Общее число единиц снаряжения всех типов, нужное для полного состава.
    pub fn total_equipment_needs(&self) -> u32 {
        self.equipment_needs.values().sum()
    }
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
        defs.extend(fra_fallback_specs().iter().filter_map(build_template));
        #[cfg(test)]
        defs.push(template(PLAIN_INFANTRY_TEMPLATE, &[(BattalionDef::infantry(), 9)]));
        Self { templates: defs.into_iter().map(|t| (t.name.clone(), t)).collect() }
    }
}

/// Шаблоны, доступные нации, в порядке показа в интерфейсе. У прочих стран — только базовая пехотная дивизия.
fn available_template_names(country: &str) -> &'static [&'static str] {
    match country {
        "GER" => &[BASE_TEMPLATE, "Infanterie-Division (mot.)", "Panzer-Division"],
        "FRA" => &[
            FRA_TEMPLATE_NAMES[0],
            FRA_TEMPLATE_NAMES[1],
            FRA_TEMPLATE_NAMES[2],
            FRA_TEMPLATE_NAMES[3],
            FRA_TEMPLATE_NAMES[4],
            FRA_TEMPLATE_NAMES[5],
            FRA_TEMPLATE_NAMES[6],
            FRA_TEMPLATE_NAMES[7],
            BASE_TEMPLATE,
        ],
        _ => &[BASE_TEMPLATE],
    }
}

impl DivisionTemplates {
    /// Реестр с шаблонами Франции из `history/units/FRA_1936.txt` игры. Нет файла или шаблон не разобран —
    /// остаётся встроенная копия (никаких паник).
    pub fn with_game_files(root: &std::path::Path) -> Self {
        let mut registry = Self::default();
        let path = root.join("history").join("units").join("FRA_1936.txt");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("[ПРЕДУПРЕЖДЕНИЕ] шаблоны FRA: {:?} не прочитан, используются встроенные", path);
            return registry;
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut loaded = 0;
        for spec in parse_division_templates(text.trim_start_matches('\u{feff}')) {
            // заменяем только известные нации шаблоны
            if !FRA_TEMPLATE_NAMES.contains(&spec.name.as_str()) {
                continue;
            }
            if let Some(template) = build_template(&spec) {
                registry.templates.insert(spec.name.clone(), template);
                loaded += 1;
            }
        }
        println!("[ШАБЛОН] FRA: прочитано шаблонов из FRA_1936.txt: {loaded}");
        registry
    }

    /// Шаблон по имени или базовый, если такого нет в реестре.
    pub fn get_or_base(&self, name: &str) -> &DivisionTemplate {
        self.templates
            .get(name)
            .or_else(|| self.templates.get(BASE_TEMPLATE))
            .expect("базовый шаблон всегда зарегистрирован")
    }

    /// Шаблон строго по имени (без подстановки базового): имя -> набор батальонов attalions.
    pub fn get(&self, name: &str) -> Option<&DivisionTemplate> {
        self.templates.get(name)
    }

    /// Имена шаблонов, которые нация может применить к своим дивизиям (только зарегистрированные).
    pub fn available_names(&self, country: &str) -> Vec<&'static str> {
        available_template_names(country).iter().copied().filter(|n| self.templates.contains_key(*n)).collect()
    }

    /// Доступен ли шаблон нации.
    pub fn is_available(&self, country: &str, name: &str) -> bool {
        self.available_names(country).contains(&name)
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
        assert_eq!(stats.equipment_needs, BTreeMap::from([(EquipmentType::InfantryEquipment, 900)]));
        assert_eq!(stats.total_equipment_needs(), 900);
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
        assert_eq!((base.max_strength, base.max_org, base.defense), (900, 60, 225));
        // потребность в снаряжении по типам: 900 пехотного, 10 оснащения сапёров, 24 орудия
        assert_eq!(
            base.equipment_needs,
            BTreeMap::from([
                (EquipmentType::InfantryEquipment, 900),
                (EquipmentType::SupportEquipment, 10),
                (EquipmentType::Artillery, 24),
            ])
        );
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
    fn equipment_needs_are_aggregated_by_type() {
        // потребность батальонов: пехота 100, лёгкий танк 50, средний 50, артиллерия 24, сапёры 10
        assert_eq!(BattalionDef::infantry().equipment_needs, (EquipmentType::InfantryEquipment, 100));
        assert_eq!(BattalionDef::light_tank().equipment_needs, (EquipmentType::LightTank, 50));
        assert_eq!(BattalionDef::medium_tank().equipment_needs, (EquipmentType::MediumTank, 50));
        assert_eq!(BattalionDef::support_artillery().equipment_needs, (EquipmentType::Artillery, 24));
        assert_eq!(BattalionDef::engineers().equipment_needs, (EquipmentType::SupportEquipment, 10));
        // танковая дивизия: 4 лёгких танка по 50 и 2 моторизованных батальона на пехотном снаряжении
        let panzer = stats_of("Panzer-Division");
        assert_eq!(
            panzer.equipment_needs,
            BTreeMap::from([(EquipmentType::InfantryEquipment, 200), (EquipmentType::LightTank, 200)])
        );
        // сапёры и разведка делят один тип оснащения: 10 + 5
        let both = template("s", &[(BattalionDef::engineers(), 1), (BattalionDef::support_recon(), 1)]).calculate_stats();
        assert_eq!(both.equipment_needs, BTreeMap::from([(EquipmentType::SupportEquipment, 15)]));
        // пустой шаблон ничего не требует
        assert!(template("empty", &[]).calculate_stats().equipment_needs.is_empty());
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
    fn registry_maps_names_to_battalions_and_limits_templates_per_nation() {
        let registry = DivisionTemplates::default();
        // имя -> набор батальонов: 9 пехотных + сапёры + артиллерия = 11 записей
        assert_eq!(registry.get(BASE_TEMPLATE).map(|t| t.battalions.len()), Some(11));
        assert_eq!(registry.get("Panzer-Division").map(|t| t.battalions.len()), Some(6));
        assert!(registry.get("Нет такого").is_none(), "get не подставляет базовый шаблон");
        // у Германии есть танковая дивизия, у Франции — нет; прочим странам доступна только пехота
        assert_eq!(registry.available_names("GER"), vec![BASE_TEMPLATE, "Infanterie-Division (mot.)", "Panzer-Division"]);
        let mut fra: Vec<&str> = FRA_TEMPLATE_NAMES.to_vec();
        fra.push(BASE_TEMPLATE);
        assert_eq!(registry.available_names("FRA"), fra);
        assert_eq!(registry.available_names("ITA"), vec![BASE_TEMPLATE]);
        assert!(registry.is_available("GER", "Panzer-Division"));
        assert!(!registry.is_available("FRA", "Panzer-Division"));
        // тестовый шаблон без поддержки в списки наций не попадает
        assert!(!registry.is_available("GER", PLAIN_INFANTRY_TEMPLATE));
    }

    #[test]
    fn medium_tank_armor_uses_weighted_rule() {
        // 3 средних танка + 3 пехотных: макс. 50, средняя 25 -> (50 * 30 + 25 * 70) / 100 = 32
        let stats = template("m", &[(BattalionDef::medium_tank(), 3), (BattalionDef::infantry(), 3)]).calculate_stats();
        assert_eq!(stats.armor, 32);
    }
}

#[cfg(test)]
mod fra_template_tests {
    use super::*;
    use std::path::Path;

    const GAME: &str = "D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV";

    fn symbol(registry: &DivisionTemplates, name: &str) -> NatoSymbol {
        registry.get(name).map(DivisionTemplate::nato_symbol).unwrap_or_default()
    }

    #[test]
    fn parser_reads_names_regiments_and_support() {
        let text = "division_template = {\n name = \"Division d'Infanterie\" # комментарий\n division_names_group = FRA_INF_01\n regiments = {\n  infantry = { x = 0 y = 0 }\n  infantry = { x = 0 y = 1 }\n }\n support = {\n  artillery = { x = 0 y = 0 }\n  engineer = { x = 0 y = 1 }\n }\n}\ndivision_template = {\n name = \"Brigade\"\n regiments = { cavalry = { x = 0 y = 0 } }\n priority = 0\n}\nunits = { division = { division_template = \"Brigade\" } }";
        let specs = parse_division_templates(text);
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "Division d'Infanterie");
        assert_eq!(specs[0].regiments, vec!["infantry", "infantry"]);
        assert_eq!(specs[0].support, vec!["artillery", "engineer"]);
        assert_eq!((specs[1].name.as_str(), specs[1].regiments.len(), specs[1].support.len()), ("Brigade", 1, 0));
    }

    #[test]
    fn broken_scripts_and_unknown_kinds_never_panic() {
        assert!(parse_division_templates("").is_empty());
        assert!(parse_division_templates("division_template = { name = ").is_empty());
        assert!(parse_division_templates("}}}} = = { {").is_empty());
        let unknown = TemplateSpec { name: "X".into(), regiments: vec!["spaceship".into()], support: vec![] };
        assert!(build_template(&unknown).is_none());
        let empty = TemplateSpec { name: "Y".into(), ..Default::default() };
        assert!(build_template(&empty).is_none());
        // нет папки игры -> встроенные шаблоны
        let registry = DivisionTemplates::with_game_files(Path::new("Z:\\нет\\игры"));
        assert!(FRA_TEMPLATE_NAMES.iter().all(|n| registry.get(n).is_some()));
    }

    #[test]
    fn french_templates_are_available_to_france_only() {
        let registry = DivisionTemplates::default();
        for name in FRA_TEMPLATE_NAMES {
            assert!(registry.is_available("FRA", name), "{name}");
            assert!(!registry.is_available("GER", name), "{name} у Германии");
        }
    }

    #[test]
    fn french_templates_have_the_right_nato_symbols() {
        let r = DivisionTemplates::default();
        assert_eq!(symbol(&r, "Division d'Infanterie"), NatoSymbol::Infantry);
        assert_eq!(symbol(&r, "Division Coloniale"), NatoSymbol::Infantry);
        assert_eq!(symbol(&r, "Division d'Infanterie Alpine"), NatoSymbol::Infantry);
        assert_eq!(symbol(&r, "Brigade Coloniale"), NatoSymbol::Infantry);
        assert_eq!(symbol(&r, "Division d'Infanterie Motorisée"), NatoSymbol::Motorized);
        assert_eq!(symbol(&r, "Division de Cavalerie"), NatoSymbol::Cavalry);
        assert_eq!(symbol(&r, "Division Légère Mécanique"), NatoSymbol::Armored);
        assert_eq!(symbol(&r, "Brigade de Chars de Combat"), NatoSymbol::Armored);
    }

    #[test]
    fn french_templates_have_the_authentic_composition() {
        let r = DivisionTemplates::default();
        let stats = |n: &str| r.get(n).map(|t| (t.battalion_count(), t.calculate_stats())).expect("шаблон");
        let (n, di) = stats("Division d'Infanterie");
        assert_eq!((n, di.combat_width, di.has_engineers, di.max_speed), (9, 18, true, 40));
        let (n, col) = stats("Division Coloniale");
        assert_eq!((n, col.has_engineers), (9, false), "у колониальной сапёров нет");
        let (n, mot) = stats("Division d'Infanterie Motorisée");
        assert_eq!((n, mot.max_speed, mot.recon), (9, 120, 3));
        assert_eq!(stats("Division d'Infanterie Alpine").0, 12);
        let (n, cav) = stats("Division de Cavalerie");
        assert_eq!((n, cav.recon, cav.max_speed), (4, 2, 60));
        let (n, dlm) = stats("Division Légère Mécanique");
        assert_eq!((n, dlm.max_speed, dlm.armor > 0, dlm.has_engineers), (6, 80, true, true));
        assert_eq!(stats("Brigade de Chars de Combat").0, 4);
        assert_eq!(stats("Brigade Coloniale").0, 6);
    }

    /// Реальный FRA_1936.txt (пропускается без игры): все 8 шаблонов разобраны и совпадают со встроенными копиями.
    #[test]
    fn real_file_templates_match_the_builtin_copies() {
        let root = Path::new(GAME);
        if !root.exists() {
            return;
        }
        let text = std::fs::read(root.join("history/units/FRA_1936.txt")).map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default();
        let names: Vec<String> = parse_division_templates(&text).into_iter().map(|s| s.name).collect();
        for expected in FRA_TEMPLATE_NAMES {
            assert!(names.iter().any(|n| n == expected), "{expected} есть в файле");
        }
        let (builtin, real) = (DivisionTemplates::default(), DivisionTemplates::with_game_files(root));
        for name in FRA_TEMPLATE_NAMES {
            let (a, b) = (builtin.get(name).expect("встроенный"), real.get(name).expect("из файла"));
            assert_eq!(a.calculate_stats(), b.calculate_stats(), "{name}");
            assert_eq!(a.nato_symbol(), b.nato_symbol(), "{name}");
        }
    }
}
