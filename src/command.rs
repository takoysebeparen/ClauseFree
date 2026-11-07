//! Офицерский корпус: генералы, фельдмаршалы и привязка армий к фронту.
//!
//! Командир — отдельная сущность с компонентом `Commander`. Фронт ссылается на него через
//! `Frontline::commander`, а `update_command_effects` каждый кадр переносит навыки командира и
//! признак превышения лимита в дивизии фронта — боевая система читает только поля `Division`.
//! Все значения целочисленные.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::division::{CombatSet, Division};
use crate::frontline::Frontline;

/// Дивизий под началом генерала без штрафа.
pub const GENERAL_DIVISION_LIMIT: u32 = 24;
/// Армий (фронтов), которыми руководит фельдмаршал.
pub const MARSHAL_ARMY_LIMIT: u32 = 5;
/// Каждый уровень навыка даёт +2% к атаке / защите.
pub const SKILL_BONUS_PCT: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommanderRank {
    General,
    /// В стартовом пуле фельдмаршалов нет; ранг используется при расширении пула и в тестах.
    #[allow(dead_code)]
    FieldMarshal,
}

impl CommanderRank {
    /// Лимит ранга по правилам: генерал — 24 дивизии, фельдмаршал — 5 армий / фронтов.
    pub fn limit(self) -> u32 {
        match self {
            CommanderRank::General => GENERAL_DIVISION_LIMIT,
            CommanderRank::FieldMarshal => MARSHAL_ARMY_LIMIT,
        }
    }

    /// Сколько дивизий на одном фронте командир ведёт без штрафа: генерал — 24, фельдмаршал — 5 армий по 24.
    pub fn division_capacity(self) -> u32 {
        match self {
            CommanderRank::General => self.limit(),
            CommanderRank::FieldMarshal => self.limit() * GENERAL_DIVISION_LIMIT,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            CommanderRank::General => "Генерал",
            CommanderRank::FieldMarshal => "Фельдмаршал",
        }
    }
}

#[derive(Component, Debug, Clone)]
pub struct Commander {
    /// Порядковый номер в пуле: детерминированный порядок и ничья при автоназначении.
    pub id: u32,
    pub name: String,
    /// Страна командира.
    pub owner: String,
    pub rank: CommanderRank,
    /// Каждый уровень: +2% к атаке подчинённых дивизий.
    pub attack_skill: u32,
    /// Каждый уровень: +2% к защите подчинённых дивизий.
    pub defense_skill: u32,
    /// Дивизии его фронта по возрастанию id (обновляет `update_command_effects`).
    pub assigned_divisions: Vec<Entity>,
}

pub struct CommandPlugin;

impl Plugin for CommandPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_commander_pool)
            // эффекты должны быть актуальны до того, как боевая цепочка кадра посчитает урон
            .add_systems(Update, update_command_effects.before(CombatSet));
    }
}

/// Стартовый пул исторических генералов: (страна, имя, атака, защита).
const STARTING_COMMANDERS: [(&str, &str, u32, u32); 4] = [
    ("GER", "Эрвин Роммель", 4, 2),
    ("GER", "Хайнц Гудериан", 4, 1),
    ("FRA", "Морис Гамелен", 1, 3),
    ("FRA", "Шарль де Голль", 3, 2),
];

fn spawn_commander_pool(mut commands: Commands) {
    for (id, (owner, name, attack_skill, defense_skill)) in STARTING_COMMANDERS.into_iter().enumerate() {
        commands.spawn(Commander {
            id: id as u32,
            name: name.to_string(),
            owner: owner.to_string(),
            rank: CommanderRank::General,
            attack_skill,
            defense_skill,
            assigned_divisions: Vec::new(),
        });
        println!("[КОМАНДОВАНИЕ] {owner}: {name} (Атк {attack_skill}, Защ {defense_skill})");
    }
}

/// Переносит навыки командира фронта в его дивизии и помечает дивизии сверх лимита.
/// Дивизии фронта упорядочены по id: первые `capacity` в лимите, остальные получают штраф.
/// Дивизии вне фронтов с командиром получают нули.
pub fn update_command_effects(
    frontlines: Query<&Frontline>,
    mut commanders: Query<(Entity, &mut Commander)>,
    mut divisions: Query<(Entity, &mut Division)>,
) {
    // дивизия -> (навык атаки, навык защиты, сверх лимита)
    let mut effects: HashMap<Entity, (u32, u32, bool)> = HashMap::new();
    let mut active: HashSet<Entity> = HashSet::new();

    for front in &frontlines {
        let Some(commander_entity) = front.commander else { continue };
        let Ok((_, mut commander)) = commanders.get_mut(commander_entity) else { continue };
        active.insert(commander_entity);

        let mut ordered: Vec<(u32, Entity)> = front
            .assigned_divisions
            .iter()
            .filter_map(|&e| divisions.get(e).ok().map(|(_, d)| (d.id, e)))
            .collect();
        ordered.sort_unstable();
        let capacity = commander.rank.division_capacity() as usize;

        let list: Vec<Entity> = ordered.iter().map(|&(_, e)| e).collect();
        if commander.assigned_divisions != list {
            commander.assigned_divisions = list;
        }
        for (i, &(_, e)) in ordered.iter().enumerate() {
            effects.insert(e, (commander.attack_skill, commander.defense_skill, i >= capacity));
        }
    }

    // Командиры без фронта никем не командуют
    for (entity, mut commander) in &mut commanders {
        if !active.contains(&entity) && !commander.assigned_divisions.is_empty() {
            commander.assigned_divisions.clear();
        }
    }

    for (entity, mut d) in &mut divisions {
        let (attack, defense, over) = effects.get(&entity).copied().unwrap_or((0, 0, false));
        if (d.command_attack_skill, d.command_defense_skill, d.over_command_limit) != (attack, defense, over) {
            d.command_attack_skill = attack;
            d.command_defense_skill = defense;
            d.over_command_limit = over;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::{DivisionTemplates, BASE_TEMPLATE};

    fn spawn_division(app: &mut App, id: u32) -> Entity {
        let templates = DivisionTemplates::default();
        let d = Division::from_template(
            id,
            format!("Div {id}"),
            "GER".to_string(),
            1,
            BASE_TEMPLATE.to_string(),
            templates.get_or_base(BASE_TEMPLATE),
        );
        app.world_mut().spawn(d).id()
    }

    fn commander(id: u32, owner: &str, attack: u32, defense: u32) -> Commander {
        Commander {
            id,
            name: format!("Командир {id}"),
            owner: owner.to_string(),
            rank: CommanderRank::General,
            attack_skill: attack,
            defense_skill: defense,
            assigned_divisions: Vec::new(),
        }
    }

    #[test]
    fn rank_limits() {
        assert_eq!(CommanderRank::General.limit(), 24);
        assert_eq!(CommanderRank::FieldMarshal.limit(), 5);
        assert_eq!(CommanderRank::General.division_capacity(), 24);
        assert_eq!(CommanderRank::FieldMarshal.division_capacity(), 120);
    }

    #[test]
    fn starting_pool_matches_history() {
        let pool: Vec<_> = STARTING_COMMANDERS.iter().map(|c| (c.0, c.1, c.2, c.3)).collect();
        assert!(pool.contains(&("GER", "Эрвин Роммель", 4, 2)));
        assert!(pool.contains(&("GER", "Хайнц Гудериан", 4, 1)));
        assert!(pool.contains(&("FRA", "Морис Гамелен", 1, 3)));
        assert!(pool.contains(&("FRA", "Шарль де Голль", 3, 2)));
    }

    #[test]
    fn effects_propagate_and_excess_divisions_are_penalized() {
        let mut app = App::new();
        app.add_systems(Update, update_command_effects);

        let general = app.world_mut().spawn(commander(0, "GER", 4, 2)).id();
        // 26 дивизий на фронте: первые 24 (по id) в лимите, две последние сверх
        let divisions: Vec<Entity> = (1..=26).map(|id| spawn_division(&mut app, id)).collect();
        let outsider = spawn_division(&mut app, 99); // на фронт не назначена
        app.world_mut().spawn(Frontline {
            owner: "GER".into(),
            enemy: "FRA".into(),
            provinces: vec![1],
            assigned_divisions: divisions.clone(),
            commander: Some(general),
        });
        app.update();

        let world = app.world();
        for (i, &e) in divisions.iter().enumerate() {
            let d = world.get::<Division>(e).unwrap();
            assert_eq!((d.command_attack_skill, d.command_defense_skill), (4, 2));
            assert_eq!(d.over_command_limit, i >= 24, "дивизия #{} (индекс {i})", d.id);
        }
        let d = world.get::<Division>(outsider).unwrap();
        assert_eq!((d.command_attack_skill, d.command_defense_skill, d.over_command_limit), (0, 0, false));
        assert_eq!(world.get::<Commander>(general).unwrap().assigned_divisions.len(), 26);
    }

    #[test]
    fn effects_cleared_when_front_is_removed() {
        let mut app = App::new();
        app.add_systems(Update, update_command_effects);
        let general = app.world_mut().spawn(commander(0, "GER", 4, 2)).id();
        let div = spawn_division(&mut app, 1);
        let front = app
            .world_mut()
            .spawn(Frontline {
                owner: "GER".into(),
                enemy: "FRA".into(),
                provinces: vec![1],
                assigned_divisions: vec![div],
                commander: Some(general),
            })
            .id();
        app.update();
        assert_eq!(app.world().get::<Division>(div).unwrap().command_attack_skill, 4);

        app.world_mut().despawn(front);
        app.update();
        let d = app.world().get::<Division>(div).unwrap();
        assert_eq!((d.command_attack_skill, d.command_defense_skill), (0, 0));
        assert!(app.world().get::<Commander>(general).unwrap().assigned_divisions.is_empty());
    }
}
