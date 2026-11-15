//! Штабной стиль интерфейса: тёмно-стальная тема, шапка со страной (флаг 41x26 + тег) и двухцветные шкалы
//! боеготовности «Организация» / «Прочность». Только слой отображения: симуляция не затрагивается.
//!
//! Кэш флагов (`FlagTextures`, ленивая загрузка TGA с запасным цветным прямоугольником) живёт в `flags.rs`;
//! здесь он используется через `country_header`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::camera::MainCamera;
use crate::colors::CountryColors;
use crate::flags::FlagTextures;
use crate::map::{ProvinceAdjacency, ProvinceCenters, ProvinceControllerChanged, ProvinceControllers};

/// Фон окон и панелей: тёмная сталь.
pub const PANEL_FILL: egui::Color32 = egui::Color32::from_rgb(22, 24, 28);
/// Обводка окон: сталь с латунным отливом.
pub const WINDOW_STROKE: egui::Color32 = egui::Color32::from_rgb(70, 75, 85);
/// Заголовки и акценты: приглушённое золото.
pub const BRASS: egui::Color32 = egui::Color32::from_rgb(215, 185, 105);
/// Шкала организации.
pub const ORG_COLOR: egui::Color32 = egui::Color32::from_rgb(46, 139, 87);
/// Шкала прочности.
pub const STRENGTH_COLOR: egui::Color32 = egui::Color32::from_rgb(198, 120, 20);
/// Размер флага в шапках окон, пиксели (medium-флаги игры 41x26).
pub const HEADER_FLAG_SIZE: [f32; 2] = [41.0, 26.0];

/// Тёмная штабная тема egui.
pub fn military_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL_FILL;
    v.window_fill = PANEL_FILL;
    v.extreme_bg_color = egui::Color32::from_rgb(16, 18, 22);
    v.faint_bg_color = egui::Color32::from_rgb(32, 36, 42);
    v.window_stroke = egui::Stroke::new(1.0_f32, WINDOW_STROKE);
    v.hyperlink_color = BRASS;
    v.selection.bg_fill = egui::Color32::from_rgb(92, 78, 40);
    v.selection.stroke = egui::Stroke::new(1.0_f32, BRASS);
    // Кнопки — тёмный металлик, при наведении светлеют и получают латунный контур
    let steel = |fill: [u8; 3], stroke: egui::Color32| {
        (egui::Color32::from_rgb(fill[0], fill[1], fill[2]), egui::Stroke::new(1.0_f32, stroke))
    };
    let (fill, stroke) = steel([44, 49, 57], WINDOW_STROKE);
    v.widgets.inactive.bg_fill = fill;
    v.widgets.inactive.weak_bg_fill = fill;
    v.widgets.inactive.bg_stroke = stroke;
    let (fill, stroke) = steel([62, 68, 78], BRASS);
    v.widgets.hovered.bg_fill = fill;
    v.widgets.hovered.weak_bg_fill = fill;
    v.widgets.hovered.bg_stroke = stroke;
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(235, 220, 170));
    let (fill, stroke) = steel([80, 70, 40], BRASS);
    v.widgets.active.bg_fill = fill;
    v.widgets.active.weak_bg_fill = fill;
    v.widgets.active.bg_stroke = stroke;
    v.widgets.noninteractive.bg_fill = PANEL_FILL;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, WINDOW_STROKE);
    v
}

/// Доля заполнения шкалы в процентах (0..=100), целочисленно; пустой максимум даёт 0.
pub fn percent(current: u32, max: u32) -> u32 {
    if max == 0 {
        0
    } else {
        (u64::from(current.min(max)) * 100 / u64::from(max)) as u32
    }
}

/// Подпись шкалы: «Организация: 45/60 (75%)».
pub fn bar_text(label: &str, current: u32, max: u32) -> String {
    format!("{label}: {current}/{max} ({}%)", percent(current, max))
}

/// Шкала с подписью поверх.
pub fn status_bar(ui: &mut egui::Ui, label: &str, current: u32, max: u32, color: egui::Color32, width: f32) {
    ui.add(
        egui::ProgressBar::new(percent(current, max) as f32 / 100.0)
            .desired_width(width)
            .fill(color)
            .text(egui::RichText::new(bar_text(label, current, max)).color(egui::Color32::WHITE).small()),
    );
}

/// Две шкалы боеготовности дивизии: зелёная Организация и оранжево-коричневая Прочность.
pub fn readiness_bars(ui: &mut egui::Ui, org: (u32, u32), strength: (u32, u32)) {
    status_bar(ui, "Организация", org.0, org.1, ORG_COLOR, 230.0);
    status_bar(ui, "Прочность", strength.0, strength.1, STRENGTH_COLOR, 230.0);
}

/// Шапка окна: флаг 41x26 (или цветной прямоугольник нации) рядом с «[TAG] Название».
pub fn country_header(ui: &mut egui::Ui, flags: &mut FlagTextures, colors: &CountryColors, tag: &str, name: &str) {
    ui.horizontal(|ui| {
        flags.show_sized(ui, tag, colors, HEADER_FLAG_SIZE);
        ui.label(egui::RichText::new(format!("[{tag}] {name}")).color(BRASS).strong());
    });
}

/// Заголовок окна: жирный латунный текст (под ним egui рисует стальной разделитель).
pub fn window_title(text: &str) -> egui::RichText {
    egui::RichText::new(text).color(BRASS).strong()
}

/// Цвет бабла при успехе атакующего (прогресс >= 50%).
pub const BUBBLE_ATTACK: egui::Color32 = egui::Color32::from_rgb(34, 139, 34);
/// Цвет бабла, когда оборона держит рубеж (прогресс < 50%).
pub const BUBBLE_DEFENSE: egui::Color32 = egui::Color32::from_rgb(160, 30, 30);
/// Тёмный металлический обод бабла.
pub const BUBBLE_RIM: egui::Color32 = egui::Color32::from_rgb(58, 62, 70);
/// Радиус бабла в экранных пикселях (не зависит от зума).
pub const BUBBLE_RADIUS: f32 = 14.0;

/// Прогресс боя 0..=100 из средних долей организации сторон (в %): доля атакующих в сумме.
/// Обе стороны на нуле — ничья, 50.
pub fn bubble_progress(attacker_org_pct: u32, defender_org_pct: u32) -> u32 {
    let total = u64::from(attacker_org_pct) + u64::from(defender_org_pct);
    if total == 0 {
        50
    } else {
        (u64::from(attacker_org_pct) * 100 / total) as u32
    }
}

/// Цвет фона бабла по прогрессу.
pub fn bubble_color(progress: u32) -> egui::Color32 {
    if progress >= 50 {
        BUBBLE_ATTACK
    } else {
        BUBBLE_DEFENSE
    }
}

/// Ширина заполненной части микро-полоски на карте (в долях пикселя целиком из процентов).
pub fn micro_bar_width(percent_value: u32, full_width: f32) -> f32 {
    full_width * percent_value.min(100) as f32 / 100.0
}

// ---------------------------------------------------------------------------
// Названия стран на карте
// ---------------------------------------------------------------------------

/// Локализованные названия стран по тегу.
#[derive(Resource, Default, Debug, Clone)]
pub struct CountryNames(pub BTreeMap<String, String>);

/// Читает `localisation/{english,russian}/countries_l_*.yml` (русские названия перекрывают английские).
/// Нет файлов — пустая таблица: надписи будут тегами.
pub fn load_country_names(root: &Path) -> CountryNames {
    let mut names = BTreeMap::new();
    for lang in ["english", "russian"] {
        let path = root.join("localisation").join(lang).join(format!("countries_l_{lang}.yml"));
        let Ok(bytes) = std::fs::read(path) else { continue };
        for (key, value) in crate::command::parse_localisation(&String::from_utf8_lossy(&bytes)) {
            let is_tag = key.len() == 3 && key.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
            if is_tag && !value.is_empty() {
                names.insert(key, value);
            }
        }
    }
    CountryNames(names)
}

/// Надпись на карте: название заглавными (`ГЕРМАНИЯ`), без названия — тег.
pub fn label_text(tag: &str, names: &CountryNames) -> String {
    names.0.get(tag).map_or_else(|| tag.to_string(), |n| n.to_uppercase())
}

/// Надпись страны: текст, визуальный центр и размах крупнейшего связного куска в мировых координатах.
#[derive(Debug, Clone, PartialEq)]
pub struct CountryLabel {
    pub tag: String,
    pub text: String,
    pub center: Vec2,
    pub span: Vec2,
    /// Провинций в этом куске.
    pub provinces: usize,
}

/// Крупнейший связный по смежности кусок множества провинций; при равенстве — с меньшим минимальным id.
pub fn largest_cluster(provinces: &BTreeSet<u32>, neighbors: &BTreeMap<u32, BTreeSet<u32>>) -> BTreeSet<u32> {
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut best: BTreeSet<u32> = BTreeSet::new();
    for &start in provinces {
        if seen.contains(&start) {
            continue;
        }
        let mut cluster = BTreeSet::from([start]);
        let mut queue = vec![start];
        while let Some(p) = queue.pop() {
            for &n in neighbors.get(&p).into_iter().flatten() {
                if provinces.contains(&n) && cluster.insert(n) {
                    queue.push(n);
                }
            }
        }
        seen.extend(cluster.iter().copied());
        if cluster.len() > best.len() {
            best = cluster;
        }
    }
    best
}

/// Центроид (среднее центров провинций) и размах (габарит) куска. `None`, если центров нет.
pub fn cluster_geometry(cluster: &BTreeSet<u32>, centers: &HashMap<u32, Vec2>) -> Option<(Vec2, Vec2)> {
    let points: Vec<Vec2> = cluster.iter().filter_map(|p| centers.get(p).copied()).collect();
    if points.is_empty() {
        return None;
    }
    let sum = points.iter().fold(Vec2::ZERO, |acc, &p| acc + p);
    let (mut min, mut max) = (points[0], points[0]);
    for &p in &points {
        min = min.min(p);
        max = max.max(p);
    }
    Some((sum / points.len() as f32, max - min))
}

/// Надписи всех стран по текущим контроллерам: центр — центроид крупнейшего связного куска, поэтому колонии
/// и анклавы не смещают надпись метрополии. Порядок — по тегу.
pub fn compute_labels(
    controllers: &ProvinceControllers,
    adjacency: &ProvinceAdjacency,
    centers: &HashMap<u32, Vec2>,
    names: &CountryNames,
) -> Vec<CountryLabel> {
    let mut by_country: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
    for (&province, tag) in &controllers.0 {
        by_country.entry(tag.as_str()).or_default().insert(province);
    }
    by_country
        .into_iter()
        .filter_map(|(tag, provinces)| {
            let cluster = largest_cluster(&provinces, &adjacency.neighbors);
            let (center, span) = cluster_geometry(&cluster, centers)?;
            Some(CountryLabel { tag: tag.to_string(), text: label_text(tag, names), center, span, provinces: cluster.len() })
        })
        .collect()
}

/// Меньше этого размера (px) надпись не рисуется — на карте мира слишком мелкая страна.
const LABEL_MIN_FONT: f32 = 9.0;
const LABEL_MAX_FONT: f32 = 44.0;
/// Масштаб камеры (мировых единиц на пиксель), при котором надписи полностью скрыты / полностью видны.
const LABEL_HIDE_SCALE: f32 = 0.45;
const LABEL_SHOW_SCALE: f32 = 1.2;

/// Размер шрифта: пропорционален экранной ширине страны (размах / масштаб камеры) и ограничен сверху.
pub fn label_font_px(span_world_x: f32, camera_scale: f32) -> f32 {
    if camera_scale <= 0.0 || !span_world_x.is_finite() {
        return 0.0;
    }
    (span_world_x / camera_scale * 0.1).min(LABEL_MAX_FONT)
}

/// Прозрачность надписи: при тактическом приближении плавно гаснет, при отдалении — полная.
pub fn label_alpha(camera_scale: f32) -> f32 {
    ((camera_scale - LABEL_HIDE_SCALE) / (LABEL_SHOW_SCALE - LABEL_HIDE_SCALE)).clamp(0.0, 1.0)
}

/// Рисует названия стран в фоновом слое egui (под окнами, мимо мыши): тёмно-угольный текст
/// с лёгкой светлой обводкой. Надписи пересчитываются при смене контроля провинций, не чаще раза в 2 с.
fn country_labels_ui(
    mut contexts: EguiContexts,
    controllers: Res<ProvinceControllers>,
    adjacency: Option<Res<ProvinceAdjacency>>,
    centers: Option<Res<ProvinceCenters>>,
    names: Res<CountryNames>,
    cameras: Query<(&Camera, &GlobalTransform, &OrthographicProjection), With<MainCamera>>,
    mut changes: EventReader<ProvinceControllerChanged>,
    mut cache: Local<Vec<CountryLabel>>,
    mut ready: Local<bool>,
    mut cooldown: Local<u32>,
) {
    *cooldown = cooldown.saturating_sub(1);
    let dirty = changes.read().count() > 0;
    if let (Some(adjacency), Some(centers)) = (&adjacency, &centers) {
        if (!*ready && !adjacency.neighbors.is_empty()) || (*ready && dirty && *cooldown == 0) {
            *cache = compute_labels(&controllers, adjacency, &centers.centers, &names);
            *ready = true;
            *cooldown = 120;
        }
    }
    let Ok((camera, camera_tf, projection)) = cameras.get_single() else { return };
    let alpha = label_alpha(projection.scale);
    if alpha <= 0.01 || cache.is_empty() {
        return;
    }
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("country_labels")));
    let viewport = camera.logical_viewport_size().unwrap_or(Vec2::new(4096.0, 4096.0));
    for label in cache.iter() {
        let font = label_font_px(label.span.x, projection.scale);
        if font < LABEL_MIN_FONT {
            continue;
        }
        let Some(screen) = camera.world_to_viewport(camera_tf, label.center.extend(0.0)) else { continue };
        if screen.x < -200.0 || screen.y < -50.0 || screen.x > viewport.x + 200.0 || screen.y > viewport.y + 50.0 {
            continue;
        }
        let pos = egui::pos2(screen.x, screen.y);
        let font_id = egui::FontId::proportional(font);
        let to_u8 = |a: f32| (a * 255.0) as u8;
        // светлая обводка: четыре смещённых копии, затем тёмно-угольный текст
        let halo = egui::Color32::from_rgba_unmultiplied(235, 232, 220, to_u8(0.55 * alpha));
        for (dx, dy) in [(-1.0_f32, 0.0_f32), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            painter.text(pos + egui::vec2(dx, dy), egui::Align2::CENTER_CENTER, &label.text, font_id.clone(), halo);
        }
        let ink = egui::Color32::from_rgba_unmultiplied(34, 36, 40, to_u8(0.72 * alpha));
        painter.text(pos, egui::Align2::CENTER_CENTER, &label.text, font_id, ink);
    }
}

/// Применяет тему один раз, как только появляется контекст egui.
fn apply_theme(mut contexts: EguiContexts, mut done: Local<bool>) {
    if *done {
        return;
    }
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    ctx.set_visuals(military_visuals());
    *done = true;
}

/// Плагин интерфейса: штабная тема.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CountryNames>().add_systems(Update, (apply_theme, country_labels_ui));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn theme_uses_the_requested_palette() {
        let v = military_visuals();
        assert_eq!(v.panel_fill, egui::Color32::from_rgb(22, 24, 28));
        assert_eq!(v.window_fill, egui::Color32::from_rgb(22, 24, 28));
        assert_eq!(v.window_stroke.color, egui::Color32::from_rgb(70, 75, 85));
        assert_eq!(BRASS, egui::Color32::from_rgb(215, 185, 105));
        assert!(v.widgets.hovered.bg_fill != v.widgets.inactive.bg_fill, "подсветка при наведении");
    }

    #[test]
    fn bar_colors_and_percentages() {
        assert_eq!(ORG_COLOR, egui::Color32::from_rgb(46, 139, 87));
        assert_eq!(STRENGTH_COLOR, egui::Color32::from_rgb(198, 120, 20));
        assert_eq!(percent(45, 60), 75);
        assert_eq!(percent(0, 60), 0);
        assert_eq!(percent(70, 60), 100, "перебор обрезается");
        assert_eq!(percent(5, 0), 0, "нулевой максимум без паники");
        assert_eq!(percent(u32::MAX, u32::MAX), 100);
        assert_eq!(bar_text("Организация", 45, 60), "Организация: 45/60 (75%)");
    }

    #[test]
    fn micro_bar_percentages_and_widths() {
        assert_eq!(percent(0, 60), 0);
        assert_eq!(percent(30, 60), 50);
        assert_eq!(percent(60, 60), 100);
        assert_eq!(percent(7, 0), 0, "деление на ноль");
        assert_eq!(micro_bar_width(0, 26.0), 0.0);
        assert_eq!(micro_bar_width(50, 26.0), 13.0);
        assert_eq!(micro_bar_width(100, 26.0), 26.0);
        assert_eq!(micro_bar_width(250, 26.0), 26.0, "больше 100% обрезается");
    }

    #[test]
    fn bubble_progress_and_color() {
        assert_eq!(bubble_progress(50, 50), 50);
        assert_eq!(bubble_progress(75, 25), 75);
        assert_eq!(bubble_progress(0, 80), 0);
        assert_eq!(bubble_progress(100, 0), 100);
        assert_eq!(bubble_progress(0, 0), 50, "обе стороны сломлены — ничья");
        assert_eq!(bubble_color(50), egui::Color32::from_rgb(34, 139, 34));
        assert_eq!(bubble_color(100), BUBBLE_ATTACK);
        assert_eq!(bubble_color(49), egui::Color32::from_rgb(160, 30, 30));
        assert_eq!(bubble_color(0), BUBBLE_DEFENSE);
        // граница: прогресс 49 при почти равных долях
        assert_eq!(bubble_color(bubble_progress(49, 51)), BUBBLE_DEFENSE);
        assert_eq!(bubble_color(bubble_progress(51, 49)), BUBBLE_ATTACK);
    }

    #[test]
    fn window_titles_are_brass() {
        assert_eq!(window_title("Дивизия").text(), "Дивизия");
    }

    fn adjacency(pairs: &[(u32, u32)]) -> ProvinceAdjacency {
        let mut a = ProvinceAdjacency::default();
        for &(x, y) in pairs {
            a.neighbors.entry(x).or_default().insert(y);
            a.neighbors.entry(y).or_default().insert(x);
        }
        a
    }

    #[test]
    fn enclave_or_colony_does_not_shift_the_label_of_the_homeland() {
        // метрополия FRA: 1-2-3 в Европе (x 0..20); колония 9 далеко в Африке (x 1000, y -900); два куска не связаны
        let centers: HashMap<u32, Vec2> = [
            (1, Vec2::new(0.0, 0.0)),
            (2, Vec2::new(10.0, 0.0)),
            (3, Vec2::new(20.0, 0.0)),
            (9, Vec2::new(1000.0, -900.0)),
            (5, Vec2::new(500.0, 500.0)),
        ]
        .into();
        let controllers = ProvinceControllers(
            [(1, "FRA"), (2, "FRA"), (3, "FRA"), (9, "FRA"), (5, "GER")].map(|(p, t)| (p, t.to_string())).into(),
        );
        let adj = adjacency(&[(1, 2), (2, 3), (3, 5)]);
        let names = CountryNames(BTreeMap::from([("FRA".to_string(), "Франция".to_string())]));
        let labels = compute_labels(&controllers, &adj, &centers, &names);
        let fra = labels.iter().find(|l| l.tag == "FRA").expect("надпись Франции");
        assert_eq!((fra.provinces, fra.center, fra.span), (3, Vec2::new(10.0, 0.0), Vec2::new(20.0, 0.0)));
        assert_eq!(fra.text, "ФРАНЦИЯ");
        let ger = labels.iter().find(|l| l.tag == "GER").expect("надпись");
        assert_eq!((ger.text.as_str(), ger.center), ("GER", Vec2::new(500.0, 500.0)), "без названия — тег");
        // порядок по тегу, остров без центров не даёт надписи
        assert_eq!(labels.iter().map(|l| l.tag.as_str()).collect::<Vec<_>>(), vec!["FRA", "GER"]);
        let lost = ProvinceControllers([(77, "ITA".to_string())].into());
        assert!(compute_labels(&lost, &adj, &centers, &names).is_empty());
    }

    #[test]
    fn largest_cluster_ties_and_empty_input() {
        let adj = adjacency(&[(1, 2), (3, 4)]);
        let all: BTreeSet<u32> = [1, 2, 3, 4].into();
        assert_eq!(largest_cluster(&all, &adj.neighbors), BTreeSet::from([1, 2]), "ничья — меньший id");
        let bigger: BTreeSet<u32> = [1, 2, 3, 4, 5].into();
        let adj = adjacency(&[(1, 2), (3, 4), (4, 5)]);
        assert_eq!(largest_cluster(&bigger, &adj.neighbors), BTreeSet::from([3, 4, 5]));
        assert!(largest_cluster(&BTreeSet::new(), &adj.neighbors).is_empty());
        assert!(cluster_geometry(&BTreeSet::from([1]), &HashMap::new()).is_none());
    }

    #[test]
    fn label_size_and_fade_follow_the_zoom() {
        // экранная ширина 600 px -> шрифт 60, но не больше потолка 44
        assert_eq!(label_font_px(600.0, 1.0), 44.0);
        assert_eq!(label_font_px(200.0, 2.0), 10.0);
        assert!(label_font_px(200.0, 8.0) < 9.0, "далёкая мелкая страна скрыта");
        assert_eq!(label_font_px(100.0, 0.0), 0.0);
        assert_eq!(label_font_px(f32::NAN, 1.0), 0.0);
        // тактический зум гасит надписи, отдаление возвращает
        assert_eq!(label_alpha(0.3), 0.0);
        assert_eq!(label_alpha(1.2), 1.0);
        assert_eq!(label_alpha(4.0), 1.0);
        let mid = label_alpha(0.8);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn country_names_fall_back_to_tags_and_read_real_localisation() {
        let none = load_country_names(Path::new("Z:\\нет\\игры"));
        assert!(none.0.is_empty());
        assert_eq!(label_text("GER", &none), "GER");
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if root.exists() {
            let names = load_country_names(root);
            assert_eq!(label_text("GER", &names), "ГЕРМАНИЯ");
            assert_eq!(label_text("FRA", &names), "ФРАНЦИЯ");
        }
    }

    #[test]
    fn unknown_country_gets_a_fallback_flag_without_panic() {
        let ctx = egui::Context::default();
        let mut flags = FlagTextures::new(PathBuf::from("Z:\\нет\\такой\\папки"));
        let colors = CountryColors::default();
        let mut shown = false;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                country_header(ui, &mut flags, &colors, "ZZZ", "Неизвестная");
                readiness_bars(ui, (30, 60), (0, 0));
                shown = true;
            });
        });
        assert!(shown);
        assert_eq!(flags.cached(), 1, "запасная плашка закэширована");
    }
}
