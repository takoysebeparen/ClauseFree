//! Дипломатия: мир, война, право прохода. Реестр решает, может ли дивизия страны входить на территорию,
//! которую контролирует другая страна (`can_enter_territory`) — этим пользуется поиск пути A* и проверка
//! приказов ПКМ. Всё целочисленное и детерминированное: пары стран лежат в `BTreeMap`.

use std::collections::BTreeMap;

use bevy::prelude::*;

/// Отношения пары стран. По умолчанию — мир.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiplomaticRelation {
    /// Мир: чужая территория закрыта.
    #[default]
    Peace,
    /// Война: можно входить, сражаться и оккупировать.
    War,
    /// Право прохода войск по территории хозяина.
    MilitaryAccess,
}

/// Реестр дипломатии. Ключ — упорядоченная пара `(a, b)`. Война симметрична (записывается обе пары);
/// право прохода направленное: `(хозяин территории, гость) -> MilitaryAccess`.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct DiplomacyRegistry {
    pub relations: BTreeMap<(String, String), DiplomaticRelation>,
}

impl DiplomacyRegistry {
    /// Отношение `a` -> `b`; без записи — мир.
    pub fn relation(&self, a: &str, b: &str) -> DiplomaticRelation {
        self.relations.get(&(a.to_string(), b.to_string())).copied().unwrap_or_default()
    }

    /// Объявляет войну (в обе стороны). Страна сама с собой не воюет. Возвращает `true`, если статус изменился.
    pub fn declare_war(&mut self, a: &str, b: &str) -> bool {
        if a == b || self.are_at_war(a, b) {
            return false;
        }
        self.relations.insert((a.to_string(), b.to_string()), DiplomaticRelation::War);
        self.relations.insert((b.to_string(), a.to_string()), DiplomaticRelation::War);
        true
    }

    #[allow(dead_code)] // API будущих договоров (мир, право прохода); пока используется в тестах
    /// Заключает мир: убирает любые записи пары (в обе стороны).
    pub fn make_peace(&mut self, a: &str, b: &str) {
        self.relations.remove(&(a.to_string(), b.to_string()));
        self.relations.remove(&(b.to_string(), a.to_string()));
    }

    #[allow(dead_code)] // API будущих договоров (мир, право прохода); пока используется в тестах
    /// `host` разрешает проход войск `guest` по своей территории. Во время войны право не выдаётся.
    pub fn grant_access(&mut self, host: &str, guest: &str) {
        if host != guest && !self.are_at_war(host, guest) {
            self.relations.insert((host.to_string(), guest.to_string()), DiplomaticRelation::MilitaryAccess);
        }
    }

    pub fn are_at_war(&self, tag_a: &str, tag_b: &str) -> bool {
        tag_a != tag_b && self.relation(tag_a, tag_b) == DiplomaticRelation::War
    }

    /// Может ли дивизия страны `unit_owner` входить в провинцию, которую контролирует `province_controller`:
    /// своя земля, земля противника в состоянии войны или земля, открытая правом прохода. Нейтралы закрыты.
    pub fn can_enter_territory(&self, unit_owner: &str, province_controller: &str) -> bool {
        unit_owner == province_controller
            || self.are_at_war(unit_owner, province_controller)
            || self.relation(province_controller, unit_owner) == DiplomaticRelation::MilitaryAccess
    }
}

/// Проверка приказа ПКМ: цель должна быть чьей-то землёй, в которую `player` вправе войти.
/// Ошибка содержит готовое сообщение для консоли.
pub fn check_entry(diplomacy: &DiplomacyRegistry, player: &str, target_controller: Option<&str>) -> Result<(), String> {
    match target_controller {
        None => Err(format!("[ПУТЬ] Цель приказа не принадлежит ни одной стране (море или неизвестная провинция), приказ отклонён")),
        Some(controller) if diplomacy.can_enter_territory(player, controller) => Ok(()),
        Some(controller) => Err(format!(
            "[ДИПЛОМАТИЯ] Нарушение суверенитета: {player} не имеет права входа в {controller}."
        )),
    }
}

/// Плагин дипломатии: регистрирует реестр (все страны в мире по умолчанию).
pub struct DiplomacyPlugin;

impl Plugin for DiplomacyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DiplomacyRegistry>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_peace_and_own_land_is_open() {
        let d = DiplomacyRegistry::default();
        assert_eq!(d.relation("GER", "POL"), DiplomaticRelation::Peace);
        assert!(!d.are_at_war("GER", "POL"));
        assert!(d.can_enter_territory("GER", "GER"));
        assert!(!d.can_enter_territory("GER", "POL"), "нейтрал закрыт");
    }

    #[test]
    fn war_is_symmetric_and_opens_enemy_land() {
        let mut d = DiplomacyRegistry::default();
        assert!(d.declare_war("GER", "FRA"));
        assert!(!d.declare_war("FRA", "GER"), "повторное объявление ничего не меняет");
        assert!(d.are_at_war("GER", "FRA") && d.are_at_war("FRA", "GER"));
        assert!(d.can_enter_territory("GER", "FRA") && d.can_enter_territory("FRA", "GER"));
        assert!(!d.can_enter_territory("GER", "POL"), "война с Францией не открывает Польшу");
        assert!(!d.declare_war("GER", "GER"), "сама с собой страна не воюет");
        d.make_peace("FRA", "GER");
        assert!(!d.can_enter_territory("GER", "FRA"));
        assert!(d.relations.is_empty());
    }

    #[test]
    fn military_access_is_directional() {
        let mut d = DiplomacyRegistry::default();
        d.grant_access("POL", "GER");
        assert!(d.can_enter_territory("GER", "POL"), "Польша пустила Германию");
        assert!(!d.can_enter_territory("POL", "GER"), "обратного права нет");
        assert_eq!(d.relation("POL", "GER"), DiplomaticRelation::MilitaryAccess);
        // на войне право не выдаётся
        d.declare_war("GER", "SWE");
        d.grant_access("SWE", "GER");
        assert_eq!(d.relation("SWE", "GER"), DiplomaticRelation::War);
    }

    #[test]
    fn entry_check_reports_sovereignty_violation() {
        let mut d = DiplomacyRegistry::default();
        assert_eq!(
            check_entry(&d, "GER", Some("POL")),
            Err("[ДИПЛОМАТИЯ] Нарушение суверенитета: GER не имеет права входа в POL.".to_string())
        );
        assert!(check_entry(&d, "GER", Some("GER")).is_ok());
        assert!(check_entry(&d, "GER", None).is_err(), "море / неизвестная провинция");
        d.declare_war("GER", "POL");
        assert!(check_entry(&d, "GER", Some("POL")).is_ok());
    }

    #[test]
    fn registry_order_is_deterministic() {
        let mut a = DiplomacyRegistry::default();
        let mut b = DiplomacyRegistry::default();
        a.declare_war("GER", "FRA");
        a.declare_war("ITA", "ENG");
        b.declare_war("ENG", "ITA");
        b.declare_war("FRA", "GER");
        assert_eq!(a, b);
        assert_eq!(a.relations.keys().next().map(|k| k.0.as_str()), Some("ENG"));
    }
}
