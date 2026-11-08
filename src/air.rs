//! Региональное превосходство в воздухе над стейтами.
//!
//! `AirSuperiorityMap` хранит для стейта число от -100 до +100: положительное — перевес Германии,
//! отрицательное — Франции. Состояние обновляется раз в сутки (`DailyTickEvent`), а в бою
//! читается как готовое число: сторона с перевесом бьёт сильнее (до +10% к атаке), сторона под
//! вражеским воздушным давлением хуже обороняется (до -15% к защите). Всё целочисленное.

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::clock::{ClockSet, DailyTickEvent};
use crate::map::{ProvinceControllers, StateRegistry};

/// Граница шкалы превосходства: -MAX_AIR..=MAX_AIR.
pub const MAX_AIR: i32 = 100;
/// Бонус к атаке при полном превосходстве, %.
pub const AIR_ATTACK_BONUS_MAX_PCT: i32 = 10;
/// Штраф к защите под полным вражеским превосходством, %.
pub const AIR_DEFENSE_PENALTY_MAX_PCT: i32 = 15;
/// На сколько пунктов в сутки небо смещается к стороне, чьи войска держат стейт на земле.
const AIR_DRIFT_PER_DAY: i32 = 1;

/// Страна с положительным превосходством.
const POSITIVE_SIDE: &str = "GER";
/// Страна с отрицательным превосходством.
const NEGATIVE_SIDE: &str = "FRA";

/// Стартовое небо: (id стейта, превосходство). 51 — Moselland (Рейнская область), 17 — Alcase Lorraine.
const STARTING_AIR: [(u32, i32); 2] = [(51, 40), (17, 40)];

#[derive(Resource, Default, Debug)]
pub struct AirSuperiorityMap {
    /// Стейт -> превосходство в воздухе (-100..=+100). Нет записи — паритет (0).
    pub air_superiority: BTreeMap<u32, i32>,
}

impl AirSuperiorityMap {
    pub fn get(&self, state_id: u32) -> i32 {
        self.air_superiority.get(&state_id).copied().unwrap_or(0)
    }
}

/// Превосходство глазами страны: для Германии как есть, для Франции с обратным знаком, для остальных 0.
pub fn side_edge(value: i32, country: &str) -> i32 {
    match country {
        POSITIVE_SIDE => value,
        NEGATIVE_SIDE => -value,
        _ => 0,
    }
}

/// Бонус к атаке стороны с перевесом в небе: до +10% (при перевесе 100).
pub fn air_attack_bonus_pct(edge: i32) -> u32 {
    (edge.clamp(0, MAX_AIR) * AIR_ATTACK_BONUS_MAX_PCT / MAX_AIR) as u32
}

/// Штраф к защите стороны под вражеским воздушным давлением: до -15% (при перевесе противника 100).
pub fn air_defense_penalty_pct(edge: i32) -> u32 {
    ((-edge).clamp(0, MAX_AIR) * AIR_DEFENSE_PENALTY_MAX_PCT / MAX_AIR) as u32
}

/// Подпись неба для интерфейса.
pub fn air_status(value: i32) -> String {
    match value.signum() {
        1 => format!("Германия +{value}"),
        -1 => format!("Франция +{}", -value),
        _ => "паритет".to_string(),
    }
}

pub struct AirPlugin;

impl Plugin for AirPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AirSuperiorityMap>()
            .add_systems(Startup, init_air_map)
            .add_systems(Update, update_air_superiority.after(ClockSet));
    }
}

fn init_air_map(registry: Res<StateRegistry>, mut air: ResMut<AirSuperiorityMap>) {
    for (state_id, value) in STARTING_AIR {
        if registry.states.iter().any(|s| s.id == state_id) {
            air.air_superiority.insert(state_id, value);
        }
    }
    println!("[ВОЗДУХ] Стейтов с заданным небом: {}", air.air_superiority.len());
}

/// Раз в сутки небо над стейтом смещается на `AIR_DRIFT_PER_DAY` в сторону той из двух стран,
/// чьи войска контролируют больше провинций стейта; при равенстве — без изменений.
/// Значение всегда остаётся в диапазоне -100..=+100.
pub fn update_air_superiority(
    mut events: EventReader<DailyTickEvent>,
    mut air: ResMut<AirSuperiorityMap>,
    registry: Res<StateRegistry>,
    controllers: Res<ProvinceControllers>,
) {
    for _ in events.read() {
        let state_ids: Vec<u32> = air.air_superiority.keys().copied().collect();
        for state_id in state_ids {
            let Some(state) = registry.states.iter().find(|s| s.id == state_id) else { continue };
            let held_by = |country: &str| {
                state.provinces.iter().filter(|p| controllers.0.get(p).is_some_and(|c| c == country)).count()
            };
            let (positive, negative) = (held_by(POSITIVE_SIDE), held_by(NEGATIVE_SIDE));
            let drift = match positive.cmp(&negative) {
                std::cmp::Ordering::Greater => AIR_DRIFT_PER_DAY,
                std::cmp::Ordering::Less => -AIR_DRIFT_PER_DAY,
                std::cmp::Ordering::Equal => 0,
            };
            if let Some(value) = air.air_superiority.get_mut(&state_id) {
                *value = (*value + drift).clamp(-MAX_AIR, MAX_AIR);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::map::StateDef;

    #[test]
    fn edge_is_relative_to_the_side() {
        assert_eq!(side_edge(40, "GER"), 40);
        assert_eq!(side_edge(40, "FRA"), -40);
        assert_eq!(side_edge(40, "ITA"), 0, "третьи страны в воздушной войне не участвуют");
    }

    #[test]
    fn bonus_and_penalty_scale_up_to_their_caps() {
        // перевес 40: +4% к атаке; под давлением -40: -6% к защите (15 * 40 / 100)
        assert_eq!(air_attack_bonus_pct(40), 4);
        assert_eq!(air_defense_penalty_pct(-40), 6);
        // потолки: +10% и -15%
        assert_eq!(air_attack_bonus_pct(100), 10);
        assert_eq!(air_defense_penalty_pct(-100), 15);
        // чужой перевес атаку не усиливает, а свой — защиту не штрафует
        assert_eq!(air_attack_bonus_pct(-60), 0);
        assert_eq!(air_defense_penalty_pct(60), 0);
        // значения за пределами шкалы обрезаются
        assert_eq!(air_attack_bonus_pct(500), 10);
        assert_eq!(air_defense_penalty_pct(-500), 15);
    }

    #[test]
    fn status_labels() {
        assert_eq!(air_status(40), "Германия +40");
        assert_eq!(air_status(-25), "Франция +25");
        assert_eq!(air_status(0), "паритет");
    }

    fn state(id: u32, provinces: Vec<u32>) -> Arc<StateDef> {
        Arc::new(StateDef {
            id,
            name: format!("S{id}"),
            owner: "FRA".into(),
            provinces,
            victory_points: vec![],
            civilian_factories: 0,
            military_factories: 0,
            resources: Default::default(),
        })
    }

    fn app_with(controlled_by: &[(u32, &str)], start: i32) -> App {
        let mut app = App::new();
        app.add_event::<DailyTickEvent>()
            .insert_resource(StateRegistry { states: vec![state(7, vec![1, 2, 3])], ..Default::default() })
            .insert_resource(ProvinceControllers(controlled_by.iter().map(|&(p, c)| (p, c.to_string())).collect()))
            .insert_resource(AirSuperiorityMap { air_superiority: BTreeMap::from([(7, start)]) })
            .add_systems(Update, update_air_superiority);
        app
    }

    fn day(app: &mut App) {
        app.world_mut().send_event(DailyTickEvent { day: 1, month: 1, year: 1936 });
        app.update();
    }

    #[test]
    fn sky_drifts_toward_the_side_holding_the_ground() {
        let mut german = app_with(&[(1, "GER"), (2, "GER"), (3, "FRA")], 40);
        day(&mut german);
        day(&mut german);
        assert_eq!(german.world().resource::<AirSuperiorityMap>().get(7), 42);

        let mut french = app_with(&[(1, "FRA"), (2, "FRA"), (3, "GER")], 40);
        day(&mut french);
        assert_eq!(french.world().resource::<AirSuperiorityMap>().get(7), 39);

        // поровну — без изменений
        let mut split = app_with(&[(1, "FRA"), (2, "GER")], 40);
        day(&mut split);
        assert_eq!(split.world().resource::<AirSuperiorityMap>().get(7), 40);
    }

    #[test]
    fn sky_stays_within_the_scale() {
        let mut high = app_with(&[(1, "GER"), (2, "GER"), (3, "GER")], 100);
        day(&mut high);
        assert_eq!(high.world().resource::<AirSuperiorityMap>().get(7), 100);
        let mut low = app_with(&[(1, "FRA"), (2, "FRA"), (3, "FRA")], -100);
        day(&mut low);
        assert_eq!(low.world().resource::<AirSuperiorityMap>().get(7), -100);
    }

    /// Стартовые стейты существуют в реальных файлах игры (пропускается без неё).
    #[test]
    fn real_data_starting_states_exist() {
        use std::path::Path;
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let registry = crate::map::load_all_states(&root.join("history").join("states")).expect("стейты");
        for (state_id, _) in STARTING_AIR {
            assert!(registry.states.iter().any(|s| s.id == state_id), "нет стейта {state_id}");
        }
    }
}
