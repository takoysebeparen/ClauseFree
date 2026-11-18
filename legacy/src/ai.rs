//! Простой ИИ: Франция разворачивает оборонительный рубеж против Германии.
//!
//! Решение принимается один раз, на первом кадре после загрузки всех стартовых данных
//! (граф смежности и дивизии из OOB создаются в `Startup`). Сам марш идёт по часовым тикам.

use bevy::prelude::*;

use crate::command::Commander;
use crate::division::{ActiveCombat, Division, OrderMove};
use crate::frontline::{build_frontline, Frontline};
use crate::map::{ProvinceAdjacency, ProvinceControllers};

const AI_OWNER: &str = "FRA";
const AI_ENEMY: &str = "GER";

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, ai_deploy_defense);
    }
}

/// Граница FRA против GER -> `Frontline` с лучшим свободным французским командиром ->
/// свободные дивизии Франции по круговому алгоритму на рубеж.
#[allow(clippy::too_many_arguments)]
fn ai_deploy_defense(
    mut done: Local<bool>,
    mut commands: Commands,
    controllers: Res<ProvinceControllers>,
    adjacency: Res<ProvinceAdjacency>,
    divisions: Query<(Entity, &Division)>,
    combats: Query<&ActiveCombat>,
    commanders: Query<(Entity, &Commander)>,
    frontlines: Query<(Entity, &Frontline)>,
    mut orders: EventWriter<OrderMove>,
) {
    // Ждём, пока стартовые данные появятся в мире (командиры тоже создаются в Startup)
    if *done || adjacency.neighbors.is_empty() || divisions.is_empty() || commanders.is_empty() {
        return;
    }
    *done = true;

    match build_frontline(
        AI_OWNER,
        AI_ENEMY,
        &controllers,
        &adjacency,
        &divisions,
        &combats,
        &commanders,
        &frontlines,
        &mut orders,
    ) {
        Ok((frontline, sent)) => {
            println!(
                "[ИИ {AI_OWNER}] Развернут оборонительный рубеж против {AI_ENEMY}: {sent} дивизий вышли на границу"
            );
            commands.spawn(frontline);
        }
        Err(reason) => println!("[ИИ {AI_OWNER}] Рубеж против {AI_ENEMY} не развернут: {reason}"),
    }
}
