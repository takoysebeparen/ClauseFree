//! Офицерский корпус: генералы, фельдмаршалы и привязка армий к фронту.
//!
//! Командир — отдельная сущность с компонентом `Commander`. Фронт ссылается на него через
//! `Frontline::commander`, а `update_command_effects` каждый кадр переносит навыки командира и
//! признак превышения лимита в дивизии фронта — боевая система читает только поля `Division`.
//! Все значения целочисленные.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use bevy::prelude::*;

use crate::division::{CombatSet, Division};
use crate::frontline::Frontline;

/// Дивизий под началом генерала без штрафа.
pub const GENERAL_DIVISION_LIMIT: u32 = 24;
/// Армий (фронтов), которыми руководит фельдмаршал.
pub const MARSHAL_ARMY_LIMIT: u32 = 5;
/// Каждый уровень навыка даёт +2% к атаке / защите.
pub const SKILL_BONUS_PCT: u32 = 2;

/// Тактика боя, выбираемая командующим раз в сутки. Наступающие выбирают из `ATTACKER_TACTICS`,
/// обороняющиеся из `DEFENDER_TACTICS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CombatTactic {
    // Атакующие
    /// Базовая: атака +5%.
    Attack,
    /// Штурм: атака +25%, парируется отходом.
    Assault,
    /// Охват: атака +25%, парируется отходом.
    Encirclement,
    // Оборонительные
    /// Базовая: защита +5%.
    Defend,
    /// Контратака: атака защитника +25%, парируется охватом.
    CounterAttack,
    /// Тактический отход: урон атакующего -25%, урон защитника -15%.
    TacticalWithdrawal,
    // Речные (только в фазе борьбы за мост)
    /// Захват моста (атака): урон +20%, при успехе снимает речной штраф; парируется удержанием моста.
    SeizeBridge,
    /// Удержание моста (оборона): защита +20%, сохраняет речной штраф; парируется контрударом.
    HoldBridge,
    /// Контрудар за мост (оборона): урон защитника +15%, выбивает врага с уже захваченного моста.
    RetakeBridge,
    // Ближний бой (только в фазе уличных боёв)
    /// Атака в упор: урон атакующего +10%, урон обороны +5%; парируется разрывом дистанции.
    CloseQuartersAttack,
    /// Разрыв дистанции (оборона): попытка отойти из упора; парирует атаку в упор.
    CQTacticalWithdrawal,
    // Прорыв
    /// Прорыв (атака): атака +25%, урон защитника -15%; парируется засадой. Нужна бронетехника или лучший командир.
    Breakthrough,
    /// Засада (оборона): защита +25%; парирует прорыв.
    Ambush,
}

/// Длительность фазы прорыва, игровых часов.
pub const BREAKTHROUGH_PHASE_HOURS: u32 = 24;
/// Прорыв доступен атакующим со средней твёрдостью выше этого порога, %.
pub const BREAKTHROUGH_MIN_HARDNESS: u32 = 50;
/// Тактики наступающих, когда прорыв доступен.
pub const ATTACKER_TACTICS_WITH_BREAKTHROUGH: [CombatTactic; 4] =
    [CombatTactic::Attack, CombatTactic::Assault, CombatTactic::Encirclement, CombatTactic::Breakthrough];
/// Тактики обороняющихся, когда у врага есть прорыв: засада как контр-пик.
pub const DEFENDER_TACTICS_WITH_AMBUSH: [CombatTactic; 4] =
    [CombatTactic::Defend, CombatTactic::CounterAttack, CombatTactic::TacticalWithdrawal, CombatTactic::Ambush];

/// Доступен ли атакующим прорыв: средняя твёрдость выше 50% либо командующий атакующих сильнее
/// (суммарный навык выше; при равенстве решает больший id ведущего генерала).
pub fn breakthrough_available(avg_hardness: u32, att_skill: u32, def_skill: u32, att_key: u32, def_key: u32) -> bool {
    avg_hardness > BREAKTHROUGH_MIN_HARDNESS
        || att_skill > def_skill
        || (att_skill == def_skill && att_key > def_key)
}

/// Длительность фазы ближнего боя, игровых часов.
pub const CLOSE_QUARTERS_PHASE_HOURS: u32 = 24;
/// В ближнем бою обе стороны наносят на столько процентов больше урона (+15%).
pub const CLOSE_QUARTERS_DAMAGE_PCT: u32 = 115;
/// В городе вес штурма у атакующего утраивается.
pub const URBAN_ASSAULT_WEIGHT_MULT: u64 = 3;
/// Тактики наступающих в фазе ближнего боя: атака в упор или продление штурма.
pub const CQ_ATTACKER_TACTICS: [CombatTactic; 2] = [CombatTactic::CloseQuartersAttack, CombatTactic::Assault];
/// Тактики обороняющихся в фазе ближнего боя.
pub const CQ_DEFENDER_TACTICS: [CombatTactic; 2] = [CombatTactic::Defend, CombatTactic::CQTacticalWithdrawal];

/// Длительность речной фазы до проверки выхода, игровых часов.
pub const BRIDGE_PHASE_HOURS: u32 = 48;
/// Речные тактики наступающей стороны.
pub const BRIDGE_ATTACKER_TACTICS: [CombatTactic; 1] = [CombatTactic::SeizeBridge];
/// Речные тактики обороняющейся стороны.
pub const BRIDGE_DEFENDER_TACTICS: [CombatTactic; 2] = [CombatTactic::HoldBridge, CombatTactic::RetakeBridge];
/// Во сколько раз растёт вес захвата моста, если среди атакующих есть сапёры.
pub const ENGINEERS_SEIZE_WEIGHT_MULT: u64 = 2;

/// Длительность фазы тактического отхода, игровых часов.
pub const WITHDRAWAL_PHASE_HOURS: u32 = 24;
/// Во время отхода лимит ширины боя для обеих сторон делится на это число (фронт сужен на 50%).
pub const WITHDRAWAL_WIDTH_DIVISOR: u32 = 2;
/// Во время отхода урон по организации обороняющихся умножается на столько процентов (арьергардный бой).
pub const WITHDRAWAL_ORG_DAMAGE_PCT: u32 = 75;

/// Фаза сражения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CombatPhase {
    /// Стандартная базовая фаза.
    #[default]
    Default,
    /// Арьергардное сдерживание и отход: фронт вдвое уже, урон по обороняющимся слабее.
    TacticalWithdrawal,
    /// Борьба за переправу: бой через реку, тактики только речные.
    Bridge,
    /// Уличный бой в упор: фронт вдвое уже, урон выше у обеих сторон, броня не даёт преимущества.
    CloseQuarters,
    /// Стремительный прорыв: окопанность обороны урезана вдвое.
    Breakthrough,
}

impl CombatPhase {
    /// Название для интерфейса.
    pub fn title(&self) -> &'static str {
        match self {
            CombatPhase::Default => "Обычная",
            CombatPhase::TacticalWithdrawal => "Тактический отход",
            CombatPhase::Bridge => "Борьба за мост",
            CombatPhase::CloseQuarters => "Ближний бой",
            CombatPhase::Breakthrough => "Прорыв",
        }
    }
}

/// Тактики наступающей стороны.
pub const ATTACKER_TACTICS: [CombatTactic; 3] = [CombatTactic::Attack, CombatTactic::Assault, CombatTactic::Encirclement];
/// Тактики обороняющейся стороны.
pub const DEFENDER_TACTICS: [CombatTactic; 3] =
    [CombatTactic::Defend, CombatTactic::CounterAttack, CombatTactic::TacticalWithdrawal];
/// Бонус к урону стороны, чья тактика парировала тактику врага, %.
pub const COUNTER_PICK_DAMAGE_PCT: u32 = 25;

impl CombatTactic {
    /// Тактика, которая парирует текущую (`None` — парировать нечем).
    pub fn counter_tactic(&self) -> Option<CombatTactic> {
        match self {
            CombatTactic::Assault | CombatTactic::Encirclement => Some(CombatTactic::TacticalWithdrawal),
            CombatTactic::CounterAttack => Some(CombatTactic::Encirclement),
            CombatTactic::SeizeBridge => Some(CombatTactic::HoldBridge),
            CombatTactic::HoldBridge => Some(CombatTactic::RetakeBridge),
            CombatTactic::CloseQuartersAttack => Some(CombatTactic::CQTacticalWithdrawal),
            CombatTactic::Breakthrough => Some(CombatTactic::Ambush),
            CombatTactic::Attack
            | CombatTactic::Defend
            | CombatTactic::TacticalWithdrawal
            | CombatTactic::RetakeBridge
            | CombatTactic::CQTacticalWithdrawal
            | CombatTactic::Ambush => None,
        }
    }

    /// Название для интерфейса.
    pub fn title(&self) -> &'static str {
        match self {
            CombatTactic::Attack => "Атака",
            CombatTactic::Assault => "Штурм",
            CombatTactic::Encirclement => "Охват",
            CombatTactic::Defend => "Оборона",
            CombatTactic::CounterAttack => "Контратака",
            CombatTactic::TacticalWithdrawal => "Отход",
            CombatTactic::SeizeBridge => "Захват моста",
            CombatTactic::HoldBridge => "Удержание моста",
            CombatTactic::RetakeBridge => "Контрудар за мост",
            CombatTactic::CloseQuartersAttack => "Ближний бой",
            CombatTactic::CQTacticalWithdrawal => "Разрыв дистанции",
            CombatTactic::Breakthrough => "Прорыв",
            CombatTactic::Ambush => "Засада",
        }
    }
}

/// Кто кого парирует в паре тактик: `(атакующий парирован, обороняющийся парирован)`.
pub fn resolve_counters(attacker: CombatTactic, defender: CombatTactic) -> (bool, bool) {
    (attacker.counter_tactic() == Some(defender), defender.counter_tactic() == Some(attacker))
}

/// Эффекты тактик для одной стороны в процентах.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TacticMods {
    /// Прибавка к атаке стороны, %.
    pub attack_pct: u32,
    /// Прибавка к защите стороны, %.
    pub defense_pct: u32,
    /// Множитель наносимого стороной урона, % (100 — без изменений).
    pub damage_pct: u32,
}

/// Эффекты тактик стороны. Парированная сторона теряет бонусы своей тактики к атаке и защите; парирующая
/// получает +25% к урону. Отход защитника режет урон атакующего до 75% и собственный урон до 85%.
pub fn side_mods(attacker: CombatTactic, defender: CombatTactic, for_attacker: bool) -> TacticMods {
    let (att_countered, def_countered) = resolve_counters(attacker, defender);
    let (own, countered, enemy_countered) =
        if for_attacker { (attacker, att_countered, def_countered) } else { (defender, def_countered, att_countered) };
    let attack_pct = if countered {
        0
    } else {
        match own {
            CombatTactic::Attack => 5,
            CombatTactic::Assault | CombatTactic::Encirclement | CombatTactic::CounterAttack => 25,
            CombatTactic::SeizeBridge => 20,
            CombatTactic::RetakeBridge => 15,
            CombatTactic::CloseQuartersAttack => 10,
            CombatTactic::Breakthrough => 25,
            CombatTactic::Defend
            | CombatTactic::TacticalWithdrawal
            | CombatTactic::HoldBridge
            | CombatTactic::CQTacticalWithdrawal
            | CombatTactic::Ambush => 0,
        }
    };
    let defense_pct = if countered {
        0
    } else {
        match own {
            CombatTactic::Defend => 5,
            CombatTactic::HoldBridge => 20,
            CombatTactic::Ambush => 25,
            _ => 0,
        }
    };
    let mut damage_pct = 100;
    if enemy_countered {
        damage_pct = damage_pct * (100 + COUNTER_PICK_DAMAGE_PCT) / 100;
    }
    if matches!(defender, CombatTactic::TacticalWithdrawal | CombatTactic::CQTacticalWithdrawal) {
        damage_pct = damage_pct * if for_attacker { 75 } else { 85 } / 100;
    }
    // Атака в упор открывает и самого атакующего: если она не парирована, оборона бьёт на 5% сильнее
    if !for_attacker && attacker == CombatTactic::CloseQuartersAttack && !att_countered {
        damage_pct = damage_pct * 105 / 100;
    }
    // Успешный прорыв сбивает контрудар обороны: её урон на 15% ниже
    if !for_attacker && attacker == CombatTactic::Breakthrough && !att_countered {
        damage_pct = damage_pct * 85 / 100;
    }
    TacticMods { attack_pct, defense_pct, damage_pct }
}

/// Взвешенный выбор тактики из `options`: вес 1, у контр-пика к `enemy` (парирует его тактику) — 2.
/// `roll` — детерминированный хэш; результат зависит только от входов.
#[cfg(test)]
pub fn pick_tactic(options: &[CombatTactic], enemy: Option<CombatTactic>, roll: u64) -> CombatTactic {
    pick_tactic_boosted(options, enemy, roll, None)
}

/// Вес тактики при выборе: 1; контр-пик к тактике врага ×2; тактика с повышением (`boost`) ещё ×2.
#[cfg(test)]
pub fn tactic_weight(tactic: CombatTactic, enemy: Option<CombatTactic>, boost: Option<CombatTactic>) -> u64 {
    tactic_weight_scaled(tactic, enemy, boost.map(|t| (t, ENGINEERS_SEIZE_WEIGHT_MULT)))
}

/// Вес с произвольным множителем для одной тактики (`scale`): штурм в городе ×3, захват моста при сапёрах ×2.
pub fn tactic_weight_scaled(tactic: CombatTactic, enemy: Option<CombatTactic>, scale: Option<(CombatTactic, u64)>) -> u64 {
    let mut weight = 1;
    if enemy.is_some_and(|e| e.counter_tactic() == Some(tactic)) {
        weight *= 2;
    }
    if let Some((scaled, factor)) = scale {
        if scaled == tactic {
            weight *= factor;
        }
    }
    weight
}

/// Как `pick_tactic`, но тактика `boost` (захват моста при сапёрах) весит вдвое больше.
pub fn pick_tactic_boosted(
    options: &[CombatTactic],
    enemy: Option<CombatTactic>,
    roll: u64,
    boost: Option<CombatTactic>,
) -> CombatTactic {
    pick_tactic_scaled(options, enemy, roll, boost.map(|t| (t, ENGINEERS_SEIZE_WEIGHT_MULT)))
}

/// Взвешенный выбор с произвольным множителем веса одной тактики.
pub fn pick_tactic_scaled(
    options: &[CombatTactic],
    enemy: Option<CombatTactic>,
    roll: u64,
    scale: Option<(CombatTactic, u64)>,
) -> CombatTactic {
    let weight = |t: CombatTactic| -> u64 { tactic_weight_scaled(t, enemy, scale) };
    let total: u64 = options.iter().map(|&t| weight(t)).sum();
    let mut point = if total == 0 { 0 } else { roll % total };
    for &tactic in options {
        let w = weight(tactic);
        if point < w {
            return tactic;
        }
        point -= w;
    }
    options.first().copied().unwrap_or(CombatTactic::Attack)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommanderRank {
    General,
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

/// Страны, чей пул командиров читается из игры.
pub const COMMANDER_COUNTRIES: [&str; 2] = ["GER", "FRA"];

/// Запись пула: командир до появления в мире.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommanderRecord {
    pub owner: String,
    pub name: String,
    pub rank: CommanderRank,
    pub attack_skill: u32,
    pub defense_skill: u32,
}

/// Пул командиров стартовой даты (порядок: страны по `COMMANDER_COUNTRIES`, внутри — порядок файла).
#[derive(Resource, Debug, Clone, Default)]
pub struct CommanderPool(pub Vec<CommanderRecord>);

/// Командир, найденный в скрипте: ключ персонажа, ранг и навыки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommander {
    /// Ключ персонажа (`GER_erwin_rommel`) или явное имя из `create_*`.
    pub key: String,
    pub rank: CommanderRank,
    pub attack_skill: u32,
    pub defense_skill: u32,
}

/// Разбирает командиров из скрипта. Понимает оба формата:
/// - `common/characters/TAG.txt`: `characters = { ID = { name = KEY corps_commander = { attack_skill = 4 .. } } }`,
///   `field_marshal = { .. }` — маршал;
/// - `create_corps_commander = { name = ".." attack_skill = 3 defense_skill = 2 }` и `create_field_marshal` —
///   прямо в файле страны.
///
/// Если у персонажа оба блока, берётся фельдмаршал. Мусор не вызывает паник; навык без числа -> 0.
pub fn parse_commanders(text: &str) -> Vec<ParsedCommander> {
    let tokens = crate::map::tokenize(text);
    let mut stack: Vec<String> = Vec::new();
    let mut out: Vec<ParsedCommander> = Vec::new();
    // персонаж внутри `characters = { ID = {` и открытый командирский блок
    let mut character: Option<String> = None;
    let mut inline_name: Option<String> = None;
    let mut open: Option<ParsedCommander> = None;
    let rank_of = |key: &str| match key {
        "corps_commander" | "create_corps_commander" => Some(CommanderRank::General),
        "field_marshal" | "create_field_marshal" => Some(CommanderRank::FieldMarshal),
        _ => None,
    };

    for i in 0..tokens.len() {
        match tokens[i].as_str() {
            "{" => {
                let key = if i >= 2 && tokens[i - 1] == "=" { tokens[i - 2].clone() } else { String::new() };
                stack.push(key.clone());
                match (stack.as_slice(), rank_of(&key)) {
                    ([root, id], _) if root == "characters" => character = Some(id.clone()),
                    ([_, _, _], Some(rank)) if stack[0] == "characters" => {
                        let key = character.clone().unwrap_or_default();
                        open = Some(ParsedCommander { key, rank, attack_skill: 0, defense_skill: 0 });
                    }
                    ([_], Some(rank)) => {
                        inline_name = None;
                        open = Some(ParsedCommander { key: String::new(), rank, attack_skill: 0, defense_skill: 0 });
                    }
                    _ => {}
                }
            }
            "}" => {
                let closes_commander = match stack.as_slice() {
                    [_, _, last] => stack[0] == "characters" && rank_of(last).is_some(),
                    [last] => rank_of(last).is_some(),
                    _ => false,
                };
                if closes_commander {
                    if let Some(mut found) = open.take() {
                        if stack.len() == 1 {
                            found.key = inline_name.take().unwrap_or_default();
                        }
                        if !found.key.is_empty() {
                            // повторный блок того же персонажа: маршал вытесняет генерала
                            match out.iter_mut().find(|c| c.key == found.key) {
                                Some(existing) if found.rank == CommanderRank::FieldMarshal => *existing = found,
                                Some(_) => {}
                                None => out.push(found),
                            }
                        }
                    }
                }
                stack.pop();
            }
            "=" => {}
            key => {
                if tokens.get(i + 1).map(String::as_str) != Some("=") {
                    continue;
                }
                let Some(value) = tokens.get(i + 2).filter(|v| !matches!(v.as_str(), "{" | "}")) else { continue };
                let skill = value.parse::<u32>().unwrap_or(0);
                match stack.as_slice() {
                    // name = "Имя" внутри create_*
                    [_] if key == "name" && open.is_some() => inline_name = Some(value.clone()),
                    [root, _, last] if root == "characters" && rank_of(last).is_some() => match (key, open.as_mut()) {
                        ("attack_skill", Some(c)) => c.attack_skill = skill,
                        ("defense_skill", Some(c)) => c.defense_skill = skill,
                        _ => {}
                    },
                    [last] if rank_of(last).is_some() => match (key, open.as_mut()) {
                        ("attack_skill", Some(c)) => c.attack_skill = skill,
                        ("defense_skill", Some(c)) => c.defense_skill = skill,
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
    out
}

/// Персонажи, нанятые страной на старте: `recruit_character = ID` в файле истории страны.
pub fn recruited_characters(text: &str) -> BTreeSet<String> {
    let tokens = crate::map::tokenize(text);
    let mut out = BTreeSet::new();
    for i in 0..tokens.len() {
        if tokens[i] == "recruit_character" && tokens.get(i + 1).map(String::as_str) == Some("=") {
            if let Some(id) = tokens.get(i + 2).filter(|v| !matches!(v.as_str(), "{" | "}")) {
                out.insert(id.clone());
            }
        }
    }
    out
}

/// Локализация имён: `key: "текст"` / `key:0 "текст"`. Ссылки `$другой_ключ$` раскрываются.
pub fn parse_localisation(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else { continue };
        let (Some(start), Some(end)) = (rest.find('"'), rest.rfind('"')) else { continue };
        if end > start && !key.is_empty() && !key.contains(char::is_whitespace) {
            map.insert(key.to_string(), rest[start + 1..end].to_string());
        }
    }
    map
}

/// Имя персонажа: локализация (с раскрытием `$ссылок$`), иначе ключ без префикса страны («erwin rommel»).
pub fn display_name(key: &str, loc: &BTreeMap<String, String>) -> String {
    let mut text = loc.get(key).cloned();
    for _ in 0..3 {
        match text.as_deref().and_then(|t| t.strip_prefix('$')?.strip_suffix('$')) {
            Some(inner) => text = loc.get(inner).cloned(),
            None => break,
        }
    }
    match text.filter(|t| !t.is_empty() && !t.starts_with('$')) {
        Some(name) => name,
        None => key.split_once('_').map_or(key, |(_, rest)| rest).replace('_', " "),
    }
}

/// Читает файлы локализации имён (`*characters_l_russian.yml`, `ideas_l_russian.yml`; затем английские).
fn load_name_localisation(root: &Path) -> BTreeMap<String, String> {
    let mut all = BTreeMap::new();
    for (lang, suffix) in [("english", "l_english.yml"), ("russian", "l_russian.yml")] {
        let dir = root.join("localisation").join(lang);
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        files.sort();
        for path in files {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if !(name.ends_with(&format!("characters_{suffix}")) || name == format!("ideas_{suffix}")) {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&path) {
                // русские строки перекрывают английские (идут позже)
                all.extend(parse_localisation(&String::from_utf8_lossy(&bytes)));
            }
        }
    }
    all
}

/// Пул командиров страны: персонажи `common/characters/TAG.txt`, нанятые на старте (`recruit_character` в
/// файле истории страны), плюс командиры, созданные прямо в нём. Без нанятых берутся все.
fn load_country_commanders(root: &Path, tag: &str, loc: &BTreeMap<String, String>) -> Vec<CommanderRecord> {
    let read = |path: std::path::PathBuf| std::fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).to_string());
    let characters = read(root.join("common").join("characters").join(format!("{tag}.txt"))).unwrap_or_default();
    let history = std::fs::read_dir(root.join("history").join("countries"))
        .ok()
        .and_then(|dir| {
            let prefix = format!("{tag} - ");
            dir.filter_map(|e| e.ok().map(|e| e.path())).find(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".txt"))
            })
        })
        .and_then(read)
        .unwrap_or_default();

    let recruited = recruited_characters(&history);
    let mut parsed: Vec<ParsedCommander> =
        parse_commanders(&characters).into_iter().filter(|c| recruited.is_empty() || recruited.contains(&c.key)).collect();
    parsed.extend(parse_commanders(&history));
    parsed
        .into_iter()
        .map(|c| CommanderRecord {
            owner: tag.to_string(),
            name: display_name(&c.key, loc),
            rank: c.rank,
            attack_skill: c.attack_skill,
            defense_skill: c.defense_skill,
        })
        .collect()
}

/// Встроенный базовый пул (если игры нет или в ней не нашлось командиров страны).
pub fn fallback_commanders(tag: &str) -> Vec<CommanderRecord> {
    STARTING_COMMANDERS
        .iter()
        .filter(|(owner, ..)| *owner == tag)
        .map(|&(owner, name, attack_skill, defense_skill)| CommanderRecord {
            owner: owner.to_string(),
            name: name.to_string(),
            rank: CommanderRank::General,
            attack_skill,
            defense_skill,
        })
        .collect()
}

/// Реальный пул командиров GER и FRA из файлов игры; для страны без найденных командиров — встроенный.
pub fn load_commander_pool(root: &Path) -> CommanderPool {
    let loc = load_name_localisation(root);
    let mut pool = Vec::new();
    for tag in COMMANDER_COUNTRIES {
        let mut found = load_country_commanders(root, tag, &loc);
        if found.is_empty() {
            eprintln!("[ПРЕДУПРЕЖДЕНИЕ] командиры {tag}: в играх не найдены, встроенный пул");
            found = fallback_commanders(tag);
        } else {
            println!("[КОМАНДОВАНИЕ] {tag}: командиров прочитано из игры: {}", found.len());
        }
        pool.extend(found);
    }
    CommanderPool(pool)
}

/// Стартовый пул исторических генералов: (страна, имя, атака, защита).
const STARTING_COMMANDERS: [(&str, &str, u32, u32); 4] = [
    ("GER", "Эрвин Роммель", 4, 2),
    ("GER", "Хайнц Гудериан", 4, 1),
    ("FRA", "Морис Гамелен", 1, 3),
    ("FRA", "Шарль де Голль", 3, 2),
];

fn spawn_commander_pool(mut commands: Commands, pool: Option<Res<CommanderPool>>) {
    let records: Vec<CommanderRecord> = match pool.filter(|p| !p.0.is_empty()) {
        Some(pool) => pool.0.clone(),
        None => COMMANDER_COUNTRIES.iter().flat_map(|tag| fallback_commanders(tag)).collect(),
    };
    for (id, record) in records.into_iter().enumerate() {
        println!(
            "[КОМАНДОВАНИЕ] {}: {} — {} (Атк {}, Защ {})",
            record.owner,
            record.name,
            record.rank.title(),
            record.attack_skill,
            record.defense_skill
        );
        commands.spawn(Commander {
            id: id as u32,
            name: record.name,
            owner: record.owner,
            rank: record.rank,
            attack_skill: record.attack_skill,
            defense_skill: record.defense_skill,
            assigned_divisions: Vec::new(),
        });
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

#[cfg(test)]
mod tactic_tests {
    use super::*;
    use CombatTactic::*;

    #[test]
    fn counters_follow_the_canon() {
        assert_eq!(Assault.counter_tactic(), Some(TacticalWithdrawal));
        assert_eq!(Encirclement.counter_tactic(), Some(TacticalWithdrawal));
        assert_eq!(CounterAttack.counter_tactic(), Some(Encirclement));
        assert_eq!(Attack.counter_tactic(), None);
        assert_eq!(Defend.counter_tactic(), None);
        assert_eq!(TacticalWithdrawal.counter_tactic(), None);
        assert_eq!(resolve_counters(Assault, TacticalWithdrawal), (true, false));
        assert_eq!(resolve_counters(Encirclement, CounterAttack), (false, true));
        assert_eq!(resolve_counters(Attack, Defend), (false, false));
        assert_eq!(resolve_counters(Assault, CounterAttack), (false, false));
    }

    #[test]
    fn titles_are_russian() {
        let titles: Vec<&str> =
            [Attack, Assault, Encirclement, Defend, CounterAttack, TacticalWithdrawal].iter().map(CombatTactic::title).collect();
        assert_eq!(titles, vec!["Атака", "Штурм", "Охват", "Оборона", "Контратака", "Отход"]);
    }

    #[test]
    fn countered_side_loses_bonuses_and_the_counter_picker_gains_damage() {
        // штурм парирован отходом: атакующий без бонуса атаки и с уроном 75%; защитник бьёт на 125% * 85% = 106%
        assert_eq!(side_mods(Assault, TacticalWithdrawal, true), TacticMods { attack_pct: 0, defense_pct: 0, damage_pct: 75 });
        assert_eq!(side_mods(Assault, TacticalWithdrawal, false), TacticMods { attack_pct: 0, defense_pct: 0, damage_pct: 106 });
        // охват против контратаки: контратака парирована (нет +25% атаки), охват бьёт на 25% сильнее
        assert_eq!(side_mods(Encirclement, CounterAttack, false), TacticMods { attack_pct: 0, defense_pct: 0, damage_pct: 100 });
        assert_eq!(side_mods(Encirclement, CounterAttack, true), TacticMods { attack_pct: 25, defense_pct: 0, damage_pct: 125 });
        // без парирования бонусы на месте
        assert_eq!(side_mods(Assault, CounterAttack, true), TacticMods { attack_pct: 25, defense_pct: 0, damage_pct: 100 });
        assert_eq!(side_mods(Assault, CounterAttack, false), TacticMods { attack_pct: 25, defense_pct: 0, damage_pct: 100 });
        assert_eq!(side_mods(Attack, Defend, true), TacticMods { attack_pct: 5, defense_pct: 0, damage_pct: 100 });
        assert_eq!(side_mods(Attack, Defend, false), TacticMods { attack_pct: 0, defense_pct: 5, damage_pct: 100 });
        // отход без парирования (против базовой атаки): урон атакующего -25%, защитника -15%
        assert_eq!(side_mods(Attack, TacticalWithdrawal, true), TacticMods { attack_pct: 5, defense_pct: 0, damage_pct: 75 });
        assert_eq!(side_mods(Attack, TacticalWithdrawal, false), TacticMods { attack_pct: 0, defense_pct: 0, damage_pct: 85 });
    }
}

#[cfg(test)]
mod real_commander_tests {
    use super::*;

    const GAME: &str = "D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV";

    const CHARACTERS: &str = "characters = {\n\tGER_erwin_rommel = {\n\t\tname = GER_erwin_rommel\n\t\tcorps_commander = {\n\t\t\ttraits = { a b }\n\t\t\tskill = 5\n\t\t\tattack_skill = 5\n\t\t\tdefense_skill = 3\n\t\t}\n\t}\n\tGER_gerd = {\n\t\tname = GER_gerd\n\t\tcorps_commander = { attack_skill = 4 defense_skill = 3 }\n\t\tfield_marshal = { attack_skill = 4 defense_skill = 4 }\n\t}\n\tGER_adolf = {\n\t\tname = GER_adolf\n\t\tcountry_leader = { ideology = fascism }\n\t}\n}";

    #[test]
    fn parses_corps_commanders_and_field_marshals() {
        let found = parse_commanders(CHARACTERS);
        assert_eq!(found.len(), 2, "вождь без командирского блока не в пуле");
        assert_eq!(
            found[0],
            ParsedCommander { key: "GER_erwin_rommel".into(), rank: CommanderRank::General, attack_skill: 5, defense_skill: 3 }
        );
        assert_eq!(found[1].rank, CommanderRank::FieldMarshal, "маршал вытесняет генерала");
        assert_eq!((found[1].attack_skill, found[1].defense_skill), (4, 4));
    }

    #[test]
    fn parses_inline_create_commanders() {
        let text = "create_corps_commander = {\n name = \"Иван Иванов\"\n attack_skill = 3\n defense_skill = 2\n}\ncreate_field_marshal = { name = \"Пётр\" attack_skill = 1 defense_skill = oops }\ncreate_corps_commander = { attack_skill = 9 }";
        let found = parse_commanders(text);
        assert_eq!(found.len(), 2, "без имени командир не создаётся");
        assert_eq!((found[0].key.as_str(), found[0].rank, found[0].attack_skill, found[0].defense_skill), ("Иван Иванов", CommanderRank::General, 3, 2));
        assert_eq!((found[1].key.as_str(), found[1].rank, found[1].defense_skill), ("Пётр", CommanderRank::FieldMarshal, 0));
    }

    #[test]
    fn broken_scripts_never_panic() {
        assert!(parse_commanders("").is_empty());
        assert!(parse_commanders("characters = { X = { corps_commander = { attack_skill = ").is_empty());
        assert!(parse_commanders("}}} = { = {").is_empty());
        assert!(recruited_characters("recruit_character =").is_empty());
    }

    #[test]
    fn recruit_list_and_localisation_resolve_names() {
        let recruited = recruited_characters("recruit_character = A\nrecruit_character = B # c\nx = { recruit_character = C }");
        assert_eq!(recruited.into_iter().collect::<Vec<_>>(), vec!["A", "B", "C"]);
        let loc = parse_localisation("\u{feff}l_russian:\n GER_erwin_rommel: \"$erwin_rommel$\"\n # комментарий\n erwin_rommel: \"Эрвин Роммель\"\n plain:0 \"Простое\"");
        assert_eq!(display_name("GER_erwin_rommel", &loc), "Эрвин Роммель", "ссылка $..$ раскрывается");
        assert_eq!(display_name("plain", &loc), "Простое");
        assert_eq!(display_name("GER_unknown_person", &loc), "unknown person", "нет перевода -> читаемый ключ");
    }

    #[test]
    fn missing_game_falls_back_to_the_basic_pool() {
        let pool = load_commander_pool(Path::new("Z:\\нет\\игры")).0;
        assert_eq!(pool.len(), STARTING_COMMANDERS.len());
        assert!(pool.iter().any(|c| c.owner == "FRA" && c.name == "Морис Гамелен"));
        assert!(pool.iter().all(|c| c.rank == CommanderRank::General));
    }

    /// Реальные GER и FRA (пропускается без игры).
    #[test]
    fn real_pools_of_germany_and_france() {
        let root = Path::new(GAME);
        if !root.exists() {
            return;
        }
        let pool = load_commander_pool(root).0;
        let of = |tag: &str| pool.iter().filter(|c| c.owner == tag).collect::<Vec<_>>();
        let (ger, fra) = (of("GER"), of("FRA"));
        assert!(ger.len() >= 20 && fra.len() >= 8, "GER {} / FRA {}", ger.len(), fra.len());
        // порядок: сначала Германия, затем Франция
        assert_eq!(pool.iter().position(|c| c.owner == "FRA"), Some(ger.len()));
        let rommel = ger.iter().find(|c| c.name == "Эрвин Роммель").expect("Роммель");
        assert_eq!((rommel.rank, rommel.attack_skill, rommel.defense_skill), (CommanderRank::General, 5, 3));
        assert!(ger.iter().any(|c| c.rank == CommanderRank::FieldMarshal), "есть фельдмаршалы");
        let de_gaulle = fra.iter().find(|c| c.name == "Шарль де Голль").expect("де Голль");
        assert_eq!((de_gaulle.attack_skill, de_gaulle.defense_skill), (4, 4));
        assert!(fra.iter().any(|c| c.name == "Морис Гамелен" && c.rank == CommanderRank::FieldMarshal));
        assert!(pool.iter().all(|c| !c.name.is_empty() && c.attack_skill <= 9 && c.defense_skill <= 9));
    }
}
