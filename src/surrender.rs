//! Очки победы (VP) и капитуляция.
//!
//! VP берутся из `victory_points` стейтов на 1936 год. Страна теряет VP, когда провинция с VP,
//! которой она владела на старте, переходит под контроль другой страны. Потеря
//! `CAPITULATION_PERCENT`% и более стартовых VP ведёт к капитуляции. Всё считается целыми числами
//! и только по событиям `ProvinceControllerChanged`.

use std::collections::{BTreeMap, HashSet};

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::colors::CountryColors;
use crate::division::{Division, SelectedDivision};
use crate::frontline::Frontline;
use crate::map::{ProvinceControllerChanged, ProvinceControllers, StateRegistry};

/// Порог капитуляции: доля потерянных стартовых VP, %.
const CAPITULATION_PERCENT: u32 = 70;
/// Страны, прогресс которых показывается в панели.
const TRACKED_COUNTRIES: [&str; 2] = ["GER", "FRA"];

// ---------------------------------------------------------------------------
// Данные
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct CountryVp {
    /// VP стартовых владений страны.
    pub total: u32,
    /// Сколько из них сейчас под контролем другой страны.
    pub lost: u32,
}

#[derive(Resource, Default)]
pub struct NationalSurrenderState {
    /// Провинция с VP -> (стартовый владелец, очки).
    vp_provinces: BTreeMap<u32, (String, u32)>,
    pub countries: BTreeMap<String, CountryVp>,
    /// Капитулировавшие: жертва -> победитель.
    pub capitulated: BTreeMap<String, String>,
    /// Победный баннер, ожидающий закрытия: (жертва, победитель).
    banner: Option<(String, String)>,
}

impl NationalSurrenderState {
    pub fn percent_lost(&self, tag: &str) -> u32 {
        self.countries.get(tag).map_or(0, |c| if c.total == 0 { 0 } else { c.lost * 100 / c.total })
    }

    /// Пересчёт потерянных VP страны по текущим контроллерам.
    fn recompute_lost(&mut self, owner: &str, controllers: &ProvinceControllers) {
        let lost = self
            .vp_provinces
            .iter()
            .filter(|(_, (start_owner, _))| start_owner == owner)
            .filter(|(province, _)| controllers.0.get(province).is_some_and(|c| c != owner))
            .map(|(_, (_, points))| *points)
            .sum();
        if let Some(country) = self.countries.get_mut(owner) {
            country.lost = lost;
        }
    }
}

/// Страна капитулировала перед `conqueror`.
#[derive(Event, Debug, Clone)]
pub struct CountryCapitulated {
    pub victim: String,
    pub conqueror: String,
}

/// VP по провинциям и суммы по странам-владельцам на старте.
fn compute_vp(registry: &StateRegistry) -> (BTreeMap<u32, (String, u32)>, BTreeMap<String, CountryVp>) {
    let mut provinces = BTreeMap::new();
    let mut countries: BTreeMap<String, CountryVp> = BTreeMap::new();
    for state in &registry.states {
        for &(province, points) in &state.victory_points {
            provinces.insert(province, (state.owner.clone(), points));
            countries.entry(state.owner.clone()).or_default().total += points;
        }
    }
    (provinces, countries)
}

// ---------------------------------------------------------------------------
// Плагин и системы
// ---------------------------------------------------------------------------

pub struct SurrenderPlugin;

impl Plugin for SurrenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NationalSurrenderState>()
            .add_event::<CountryCapitulated>()
            .add_systems(Startup, init_surrender_state)
            .add_systems(Update, (track_victory_points, surrender_ui))
            // PostUpdate: деспавн применяется в конце кадра, до следующих систем симуляции — без гонок
            // с `resolve_combats`, который в том же кадре мог бы снести те же дивизии повторно.
            .add_systems(PostUpdate, resolve_capitulation);
    }
}

fn init_surrender_state(registry: Res<StateRegistry>, mut state: ResMut<NationalSurrenderState>) {
    let (provinces, countries) = compute_vp(&registry);
    for tag in TRACKED_COUNTRIES {
        if let Some(c) = countries.get(tag) {
            println!("[VP] {tag}: стартовых очков победы {}", c.total);
        }
    }
    state.vp_provinces = provinces;
    state.countries = countries;
}

/// Смена контроля над провинцией с VP -> пересчёт потерь -> проверка порога капитуляции.
fn track_victory_points(
    mut events: EventReader<ProvinceControllerChanged>,
    controllers: Res<ProvinceControllers>,
    mut state: ResMut<NationalSurrenderState>,
    mut capitulated: EventWriter<CountryCapitulated>,
) {
    for event in events.read() {
        let Some((owner, _)) = state.vp_provinces.get(&event.province_id).cloned() else { continue };
        state.recompute_lost(&owner, &controllers);

        if state.capitulated.contains_key(&owner) || event.new_controller == owner {
            continue;
        }
        let Some(country) = state.countries.get(&owner) else { continue };
        if country.total > 0 && country.lost * 100 >= country.total * CAPITULATION_PERCENT {
            println!("[VP] {owner}: потеряно {}/{} VP — порог капитуляции достигнут", country.lost, country.total);
            state.capitulated.insert(owner.clone(), event.new_controller.clone());
            state.banner = Some((owner.clone(), event.new_controller.clone()));
            capitulated.send(CountryCapitulated { victim: owner, conqueror: event.new_controller.clone() });
        }
    }
}

/// Капитуляция: дивизии и фронты проигравшей страны удаляются, её провинции переходят победителю.
fn resolve_capitulation(
    mut events: EventReader<CountryCapitulated>,
    mut commands: Commands,
    divisions: Query<(Entity, &Division)>,
    frontlines: Query<(Entity, &Frontline)>,
    mut controllers: ResMut<ProvinceControllers>,
    mut changed: EventWriter<ProvinceControllerChanged>,
    mut selected: ResMut<SelectedDivision>,
) {
    // Защита от двойного деспавна: каждый корень удаляется один раз за вызов
    let mut despawned: HashSet<Entity> = HashSet::new();

    for event in events.read() {
        let mut victims: Vec<(u32, Entity)> =
            divisions.iter().filter(|(_, d)| d.owner == event.victim).map(|(e, d)| (d.id, e)).collect();
        victims.sort_unstable();
        for (_, entity) in victims {
            if despawned.insert(entity) {
                commands.entity(entity).despawn_recursive();
                if selected.0 == Some(entity) {
                    selected.0 = None;
                }
            }
        }

        // Фронты, где проигравшая страна — одна из сторон, больше не нужны
        for (entity, frontline) in &frontlines {
            if (frontline.owner == event.victim || frontline.enemy == event.victim) && despawned.insert(entity) {
                commands.entity(entity).despawn();
            }
        }

        // Все провинции под контролем побеждённой страны переходят победителю (BTreeMap — порядок по id)
        let provinces: Vec<u32> =
            controllers.0.iter().filter(|(_, c)| **c == event.victim).map(|(&id, _)| id).collect();
        for province_id in provinces {
            controllers.0.insert(province_id, event.conqueror.clone());
            changed.send(ProvinceControllerChanged { province_id, new_controller: event.conqueror.clone() });
        }

        println!("[КАПИТУЛЯЦИЯ] {} капитулировала перед {}! Война завершена.", event.victim, event.conqueror);
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

fn country_name(tag: &str) -> &str {
    match tag {
        "GER" => "Германия",
        "FRA" => "Франция",
        other => other,
    }
}

fn country_name_genitive(tag: &str) -> &str {
    match tag {
        "GER" => "Германии",
        "FRA" => "Франции",
        other => other,
    }
}

fn surrender_ui(
    mut contexts: EguiContexts,
    mut state: ResMut<NationalSurrenderState>,
    colors: Res<CountryColors>,
) {
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    let frame = egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 200));

    // Прогресс капитуляции под верхней панелью экономики
    egui::Window::new("surrender_progress")
        .title_bar(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 52.0])
        .resizable(false)
        .frame(frame)
        .show(ctx, |ui| {
            for tag in TRACKED_COUNTRIES {
                let Some(c) = state.countries.get(tag) else { continue };
                let text = format!("{tag}: Капитуляция {}/{} VP ({}%)", c.lost, c.total, state.percent_lost(tag));
                if state.capitulated.contains_key(tag) {
                    ui.label(egui::RichText::new(text).color(egui::Color32::from_rgb(230, 90, 80)).strong());
                } else {
                    ui.label(egui::RichText::new(text).color(colors.ui_color32(tag)));
                }
            }
        });

    // Победное окно после капитуляции
    let mut close = false;
    if let Some((victim, conqueror)) = &state.banner {
        egui::Window::new("Победа")
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.heading(format!(
                    "{} капитулировала! Победа {}",
                    country_name(victim),
                    country_name_genitive(conqueror)
                ));
                ui.add_space(6.0);
                if ui.button("Продолжить").clicked() {
                    close = true;
                }
            });
    }
    if close {
        state.banner = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::load_all_states;
    use std::path::Path;

    /// Проверка парсинга VP на реальных файлах игры; без установленной игры тест пропускается.
    #[test]
    fn parses_real_victory_points() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let registry = load_all_states(&root.join("history").join("states")).expect("стейты читаются");
        let (provinces, countries) = compute_vp(&registry);
        println!("провинций с VP: {}", provinces.len());
        for tag in TRACKED_COUNTRIES {
            println!("{tag}: {:?}", countries.get(tag));
        }
        // Берлин (6521) и Париж (11506) по 50 VP
        assert_eq!(provinces.get(&6521).map(|(o, p)| (o.as_str(), *p)), Some(("GER", 50)));
        assert_eq!(provinces.get(&11506).map(|(o, p)| (o.as_str(), *p)), Some(("FRA", 50)));
    }
}
