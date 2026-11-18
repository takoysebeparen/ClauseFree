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
use crate::fonts::{
    build_ridge, fit_glyph_size, label_visible, layout_on_ridge, principal_axis, screen_letter_height, watermark_alpha,
    Axis, GlyphQuad, MapFont, MapFontRes, Ridge, TextMetrics,
};
use crate::map::{
    ProvinceAdjacency, ProvinceAreas, ProvinceCenters, ProvinceControllerChanged, ProvinceControllers, ProvinceMap,
};

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

/// Шапка окна: флаг 41x26 (или цветной прямоугольник нации) рядом с «[TAG] Историческое название»
/// и необязательной припиской `extra` (`| Дивизия "..."`).
pub fn country_header(
    ui: &mut egui::Ui,
    flags: &mut FlagTextures,
    colors: &CountryColors,
    names: &CountryNames,
    tag: &str,
    extra: &str,
) {
    ui.horizontal(|ui| {
        flags.show_sized(ui, tag, colors, HEADER_FLAG_SIZE);
        let text = if extra.is_empty() { names.header_text(tag) } else { format!("{} | {extra}", names.header_text(tag)) };
        ui.label(egui::RichText::new(text).color(BRASS).strong());
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
// Названия стран: история 1936 с учётом правящей партии
// ---------------------------------------------------------------------------

/// Локализованные названия стран и правящие партии на старте.
#[derive(Resource, Default, Debug, Clone)]
pub struct CountryNames {
    /// Ключи локализации: `GER`, `GER_fascism`, `GER_neutrality` -> текст.
    pub names: BTreeMap<String, String>,
    /// Тег -> `ruling_party` из `history/countries/` (на старте 1936).
    pub parties: BTreeMap<String, String>,
}

impl CountryNames {
    /// Историческое название: `<TAG>_<ruling_party>` -> `<TAG>_neutrality` -> `<TAG>` -> сам тег.
    pub fn title(&self, tag: &str) -> String {
        let party_key = self.parties.get(tag).map(|party| format!("{tag}_{party}"));
        let candidates = [party_key, Some(format!("{tag}_neutrality")), Some(tag.to_string())];
        candidates.into_iter().flatten().find_map(|key| self.names.get(&key).cloned()).unwrap_or_else(|| tag.to_string())
    }

    /// Строка шапки окна: `[GER] Третий рейх`.
    pub fn header_text(&self, tag: &str) -> String {
        format!("[{tag}] {}", self.title(tag))
    }
}

/// Ключ локализации страны: `GER` или `GER_fascism` (теги заглавными, идеология строчными).
fn is_country_key(key: &str) -> bool {
    let Some(tag) = key.get(..3) else { return false };
    if !tag.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) {
        return false;
    }
    match key.get(3..) {
        Some("") => true,
        Some(rest) => {
            rest.strip_prefix('_').is_some_and(|party| {
                !party.is_empty() && party.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            })
        }
        None => false,
    }
}

/// Читает `localisation/{english,russian}/countries_l_*.yml` (русские названия перекрывают английские) и
/// правящие партии стран. Нет файлов — пустые таблицы: надписи будут тегами.
pub fn load_country_names(root: &Path) -> CountryNames {
    let mut names = BTreeMap::new();
    for lang in ["english", "russian"] {
        let path = root.join("localisation").join(lang).join(format!("countries_l_{lang}.yml"));
        let Ok(bytes) = std::fs::read(path) else { continue };
        for (key, value) in crate::command::parse_localisation(&String::from_utf8_lossy(&bytes)) {
            if is_country_key(&key) && !value.is_empty() {
                names.insert(key, value);
            }
        }
    }
    CountryNames { names, parties: crate::flags::load_ruling_parties(root) }
}

/// Надпись на карте: историческое название заглавными (`ТРЕТИЙ РЕЙХ`), без названия — тег.
pub fn label_text(tag: &str, names: &CountryNames) -> String {
    names.title(tag).to_uppercase()
}

// ---------------------------------------------------------------------------
// Названия стран на карте
// ---------------------------------------------------------------------------

/// Отдельный кластер провинций получает свою подпись, если в нём не меньше стольких провинций.
pub const SECONDARY_LABEL_MIN_PROVINCES: usize = 25;
/// Кегль подписи отдельного кластера относительно столичной, %.
pub const SECONDARY_LABEL_FONT_PCT: f32 = 80.0;

/// Надпись страны: текст, территория (ось и хребет) и мировой размер глифа.
#[derive(Debug, Clone, PartialEq)]
pub struct CountryLabel {
    pub tag: String,
    pub text: String,
    /// Центроид центров провинций кластера.
    pub center: Vec2,
    /// Габарит кластера по осям карты.
    pub span: Vec2,
    /// Провинций в этом куске.
    pub provinces: usize,
    /// Подпись крупной колонии (кегль 80%), а не главная.
    pub secondary: bool,
    /// Главная ось инерции и размеры территории.
    pub axis: Axis,
    /// Изогнутый хребет, вдоль которого стоят буквы.
    pub ridge: Ridge,
    /// Высота строки в мировых координатах карты (для колонии уже умножена на 0.8).
    pub glyph_size: f32,
    /// Высота заглавных в долях строки (для экранной высоты букв).
    pub cap_ratio: f32,
}

/// Связные по смежности компоненты множества провинций: сначала крупнейшие, при равенстве — с меньшим id.
pub fn components(provinces: &BTreeSet<u32>, neighbors: &BTreeMap<u32, BTreeSet<u32>>) -> Vec<BTreeSet<u32>> {
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut out: Vec<BTreeSet<u32>> = Vec::new();
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
        out.push(cluster);
    }
    out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.iter().next().cmp(&b.iter().next())));
    out
}

/// Крупнейший связный кусок множества провинций.
#[cfg(test)]
pub fn largest_cluster(provinces: &BTreeSet<u32>, neighbors: &BTreeMap<u32, BTreeSet<u32>>) -> BTreeSet<u32> {
    components(provinces, neighbors).into_iter().next().unwrap_or_default()
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

/// Надписи стран по текущим контроллерам. Главная подпись — на связном куске со столицей нации (если
/// столица под её контролем), иначе на крупнейшем; отдельные кластеры без сухопутной связи с ним
/// (колонии, заморские владения) из `SECONDARY_LABEL_MIN_PROVINCES` провинций и больше получают вторую подпись.
/// Для каждого куска считаются главная ось (PCA), хребет и мировой размер глифа, при котором слово влезает
/// в `0.75 L` и `0.65 W`. Порядок — по тегу, главная подпись первой.
pub fn compute_labels(
    controllers: &ProvinceControllers,
    adjacency: &ProvinceAdjacency,
    centers: &HashMap<u32, Vec2>,
    names: &CountryNames,
    capitals: &BTreeMap<String, u32>,
    font: Option<&MapFont>,
    area_of: &dyn Fn(u32) -> f32,
    locate: &dyn Fn(Vec2) -> Option<u32>,
) -> Vec<CountryLabel> {
    let mut by_country: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
    for (&province, tag) in &controllers.0 {
        by_country.entry(tag.as_str()).or_default().insert(province);
    }
    let mut labels = Vec::new();
    for (tag, provinces) in by_country {
        let parts = components(&provinces, &adjacency.neighbors);
        let capital_part = capitals.get(tag).and_then(|c| parts.iter().position(|p| p.contains(c)));
        let main = capital_part.unwrap_or(0);
        let text = label_text(tag, names);
        let metrics = TextMetrics::of(font, &text);
        let build = |cluster: &BTreeSet<u32>, secondary: bool| -> Option<CountryLabel> {
            let (center, span) = cluster_geometry(cluster, centers)?;
            let placed: Vec<(Vec2, f32)> =
                cluster.iter().filter_map(|p| centers.get(p).map(|&c| (c, area_of(*p)))).collect();
            let points: Vec<Vec2> = placed.iter().map(|&(c, _)| c).collect();
            let weights: Vec<f32> = placed.iter().map(|&(_, a)| a).collect();
            let axis = principal_axis(&points)?;
            let ridge = build_ridge(&points, &weights, &axis);
            let fitted = fit_glyph_size(&axis, ridge.length(), &metrics);
            let mut glyph_size = if secondary { fitted * SECONDARY_LABEL_FONT_PCT / 100.0 } else { fitted };
            // Буквы строго внутри страны: пока первая, средняя или последняя вылезают за территорию — кегль -10%
            if let Some(f) = font {
                for _ in 0..LABEL_SHRINK_STEPS {
                    let quads = layout_on_ridge(&f.font, &text, &ridge, glyph_size);
                    if letters_inside(&quads, tag, controllers, locate) {
                        break;
                    }
                    glyph_size *= LABEL_SHRINK_FACTOR;
                }
            }
            Some(CountryLabel {
                tag: tag.to_string(),
                text: text.clone(),
                center,
                span,
                provinces: cluster.len(),
                secondary,
                axis,
                ridge,
                glyph_size,
                cap_ratio: metrics.cap_ratio,
            })
        };
        labels.extend(parts.get(main).and_then(|c| build(c, false)));
        for (i, cluster) in parts.iter().enumerate() {
            if i != main && cluster.len() >= SECONDARY_LABEL_MIN_PROVINCES {
                labels.extend(build(cluster, true));
            }
        }
    }
    labels
}

/// Цвет тела букв: тёмно-угольный, альфа задаётся зумом.
pub const LABEL_INK: [u8; 3] = [22, 24, 28];
/// Мягкий светлый ореол вокруг букв и его доля от альфы тела.
pub const LABEL_HALO: [u8; 3] = [235, 225, 200];
pub const LABEL_HALO_ALPHA: f32 = 0.45;
/// Сколько раз кегль можно уменьшить на 10%, пока буквы не лягут внутрь страны.
pub const LABEL_SHRINK_STEPS: u32 = 5;
pub const LABEL_SHRINK_FACTOR: f32 = 0.9;

/// Лежат ли первая, средняя и последняя буквы (все углы и центр квадрата) в провинциях этой страны.
pub fn letters_inside(
    quads: &[GlyphQuad],
    tag: &str,
    controllers: &ProvinceControllers,
    locate: &dyn Fn(Vec2) -> Option<u32>,
) -> bool {
    let owned = |point: Vec2| locate(point).is_some_and(|p| controllers.0.get(&p).is_some_and(|c| c == tag));
    let picks = [quads.first(), quads.get(quads.len() / 2), quads.last()];
    picks.into_iter().flatten().all(|q| {
        let center = (q.corners[0] + q.corners[2]) * 0.5;
        owned(center) && q.corners.iter().all(|&c| owned(c))
    })
}

/// Вершины квадратов букв в экранных координатах egui (мир -> экран: сдвиг `origin`, деление на масштаб камеры,
/// ось Y вниз). Текстура — белый атлас с покрытием в альфа-канале, цвет — тёмно-угольный с альфой зума.
fn glyph_mesh(quads: &[GlyphQuad], origin: Vec2, scale: f32, texture: egui::TextureId, color: egui::Color32, mesh: &mut egui::Mesh) {
    mesh.texture_id = texture;
    for q in quads {
        let base = mesh.vertices.len() as u32;
        for (corner, uv) in q.corners.iter().zip(q.uv.iter()) {
            mesh.vertices.push(egui::epaint::Vertex {
                pos: egui::pos2(origin.x + corner.x / scale, origin.y - corner.y / scale),
                uv: egui::pos2(uv.x, uv.y),
                color,
            });
        }
        mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Рисует названия стран в фоновом слое egui (под окнами, мимо мыши): оригинальным шрифтом карты, буква за буквой
/// вдоль хребта территории; без шрифта игры — шрифтом egui с поворотом. Размер задан в мировых координатах,
/// поэтому буквы растут вместе с провинциями; при приближении надпись становится водяным знаком, но не пропадает.
/// Надписи пересчитываются при смене контроля, не чаще раза в 2 с.
#[allow(clippy::too_many_arguments)]
fn country_labels_ui(
    mut contexts: EguiContexts,
    controllers: Res<ProvinceControllers>,
    adjacency: Option<Res<ProvinceAdjacency>>,
    centers: Option<Res<ProvinceCenters>>,
    network: Option<Res<crate::economy::SupplyNetwork>>,
    names: Res<CountryNames>,
    font: Res<MapFontRes>,
    map: Option<Res<ProvinceMap>>,
    areas: Option<Res<ProvinceAreas>>,
    cameras: Query<(&Camera, &GlobalTransform, &OrthographicProjection), With<MainCamera>>,
    mut changes: EventReader<ProvinceControllerChanged>,
    mut cache: Local<Vec<CountryLabel>>,
    mut ready: Local<bool>,
    mut cooldown: Local<u32>,
    mut atlas: Local<Option<(egui::TextureHandle, egui::TextureHandle)>>,
) {
    *cooldown = cooldown.saturating_sub(1);
    let dirty = changes.read().count() > 0;
    if let (Some(adjacency), Some(centers)) = (&adjacency, &centers) {
        if (!*ready && !adjacency.neighbors.is_empty()) || (*ready && dirty && *cooldown == 0) {
            let capitals = network.as_ref().map(|n| n.capitals.clone()).unwrap_or_default();
            let area_of = |p: u32| areas.as_ref().and_then(|a| a.0.get(&p)).map_or(1.0, |&n| n as f32);
            let locate = |w: Vec2| {
                let map = map.as_ref()?;
                let (x, y) = map.world_to_pixel(w)?;
                map.province_at_pixel(x, y).map(|p| p.id)
            };
            *cache = compute_labels(&controllers, adjacency, &centers.centers, &names, &capitals, font.0.as_ref(), &area_of, &locate);
            *ready = true;
            *cooldown = 120;
        }
    }
    let Ok((camera, camera_tf, projection)) = cameras.get_single() else { return };
    let scale = projection.scale;
    if cache.is_empty() || !(scale > 0.0 && scale.is_finite()) {
        return;
    }
    let Some(origin) = camera.world_to_viewport(camera_tf, Vec3::ZERO) else { return };
    let Some(ctx) = contexts.try_ctx_mut() else { return };
    let viewport = camera.logical_viewport_size().unwrap_or(Vec2::new(4096.0, 4096.0));
    let alpha = watermark_alpha(scale);
    if alpha < 0.01 {
        return; // планетарный зум: названия растворились
    }
    let ink = egui::Color32::from_rgba_unmultiplied(LABEL_INK[0], LABEL_INK[1], LABEL_INK[2], (alpha * 255.0) as u8);
    let halo =
        egui::Color32::from_rgba_unmultiplied(LABEL_HALO[0], LABEL_HALO[1], LABEL_HALO[2], (alpha * LABEL_HALO_ALPHA * 255.0) as u8);

    // атлас карты в две текстуры egui (белый цвет): покрытие тела букв и покрытие светлого ореола
    if atlas.is_none() {
        if let Some(f) = &font.0 {
            let make = |name: &str, layer: &[u8]| {
                let rgba: Vec<u8> = layer.iter().flat_map(|&c| [255, 255, 255, c]).collect();
                let image = egui::ColorImage::from_rgba_unmultiplied([f.width, f.height], &rgba);
                ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
            };
            *atlas = Some((make("map_font_body", &f.coverage), make("map_font_halo", &f.halo)));
        }
    }
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Background, egui::Id::new("country_labels")));
    let world_to_screen = |w: Vec2| Vec2::new(origin.x + w.x / scale, origin.y - w.y / scale);
    let (mut body_mesh, mut halo_mesh) = (egui::Mesh::default(), egui::Mesh::default());

    for label in cache.iter() {
        if !label_visible(screen_letter_height(label.glyph_size, label.cap_ratio, scale)) {
            continue;
        }
        let reach = label.axis.length.max(label.span.x).max(label.span.y) * 0.5;
        let c = world_to_screen(label.axis.center);
        let margin = reach / scale + 40.0;
        if c.x < -margin || c.y < -margin || c.x > viewport.x + margin || c.y > viewport.y + margin {
            continue;
        }
        match (&font.0, atlas.as_ref()) {
            (Some(f), Some((body, light))) => {
                let quads = layout_on_ridge(&f.font, &label.text, &label.ridge, label.glyph_size);
                glyph_mesh(&quads, origin, scale, light.id(), halo, &mut halo_mesh);
                glyph_mesh(&quads, origin, scale, body.id(), ink, &mut body_mesh);
            }
            _ => {
                // запасной путь: шрифт egui, слово целиком вдоль оси с поворотом
                let px = (label.glyph_size / scale).clamp(1.0, 400.0);
                let angle = -label.axis.angle;
                let (sin, cos) = angle.sin_cos();
                let mid = world_to_screen(label.ridge.at(label.ridge.length() * 0.5).0);
                // двухпроходный рендер: светлая подложка со сдвигом 1.5 px, затем тёмное тело
                for (color, dx, dy) in [(halo, -1.5_f32, -1.5_f32), (halo, 1.5, 1.5), (halo, -1.5, 1.5), (halo, 1.5, -1.5), (ink, 0.0, 0.0)] {
                    let galley = painter.layout_no_wrap(label.text.clone(), egui::FontId::proportional(px), color);
                    let (w, h) = (galley.size().x, galley.size().y);
                    let top_left = egui::pos2(
                        mid.x + dx - (w * 0.5 * cos - h * 0.5 * sin),
                        mid.y + dy - (w * 0.5 * sin + h * 0.5 * cos),
                    );
                    painter.add(egui::epaint::TextShape::new(top_left, galley, color).with_angle(angle));
                }
            }
        }
    }
    // сначала светлый ореол, поверх — тёмное тело букв
    for mesh in [halo_mesh, body_mesh] {
        if !mesh.vertices.is_empty() {
            painter.add(egui::Shape::mesh(mesh));
        }
    }
}

// ---------------------------------------------------------------------------
// Оригинальные иконки интерфейса (.dds)
// ---------------------------------------------------------------------------

/// Иконка интерфейса игры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IconKey {
    CivFactory,
    MilFactory,
    Manpower,
    Resource(crate::economy::ResourceType),
}

/// Файл и (кадр, число кадров) иконки: фабрики и людские ресурсы — `gfx/texticons`, ресурсы — кадры
/// полосы `GFX_resources_strip` (`interface/general_stuff.gfx`: `gfx/interface/resources_strip.dds`, 7 кадров).
pub fn icon_source(key: IconKey) -> (&'static str, (u32, u32)) {
    use crate::economy::ResourceType as R;
    const STRIP: &str = "gfx/interface/resources_strip.dds";
    match key {
        IconKey::CivFactory => ("gfx/texticons/civ_factory.dds", (0, 1)),
        IconKey::MilFactory => ("gfx/texticons/mil_factory.dds", (0, 1)),
        IconKey::Manpower => ("gfx/texticons/manpower_texticon.dds", (0, 1)),
        IconKey::Resource(R::Oil) => (STRIP, (0, 7)),
        IconKey::Resource(R::Aluminium) => (STRIP, (1, 7)),
        IconKey::Resource(R::Rubber) => (STRIP, (2, 7)),
        IconKey::Resource(R::Tungsten) => (STRIP, (3, 7)),
        IconKey::Resource(R::Steel) => (STRIP, (4, 7)),
    }
}

/// Декодированная иконка: RGBA8 построчно сверху вниз.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconPixels {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Декодирует несжатый 32-битный `.dds` (маски каналов из заголовка). Сжатые форматы (DXT), мусор и
/// обрезанные файлы дают `None`.
pub fn decode_dds(bytes: &[u8]) -> Option<IconPixels> {
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if bytes.get(..4)? != b"DDS " || word(4)? != 124 {
        return None;
    }
    let (height, width) = (word(12)? as usize, word(16)? as usize);
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return None;
    }
    let (fourcc, bits) = (word(84)?, word(88)?);
    let pixel_flags = word(80)?;
    // 0x4 = FOURCC (сжатый формат), 0x40 = RGB
    if pixel_flags & 0x4 != 0 || fourcc != 0 || bits != 32 {
        return None;
    }
    let masks = [word(92)?, word(96)?, word(100)?, word(104)?];
    if masks[..3].iter().any(|&m| m == 0) {
        return None;
    }
    let data = bytes.get(128..128usize.checked_add(width.checked_mul(height)?.checked_mul(4)?)?)?;
    let extract = |value: u32, mask: u32| if mask == 0 { 255 } else { ((value & mask) >> mask.trailing_zeros()) as u8 };
    let mut rgba = Vec::with_capacity(width * height * 4);
    for px in data.chunks_exact(4) {
        let v = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
        rgba.extend_from_slice(&[extract(v, masks[0]), extract(v, masks[1]), extract(v, masks[2]), extract(v, masks[3])]);
    }
    Some(IconPixels { width, height, rgba })
}

/// Вырезает кадр `(индекс, всего)` из горизонтальной полосы. Кадр вне полосы — `None`.
pub fn crop_frame(strip: &IconPixels, frame: (u32, u32)) -> Option<IconPixels> {
    let (index, count) = (frame.0 as usize, frame.1 as usize);
    if count == 0 || index >= count {
        return None;
    }
    if count == 1 {
        return Some(strip.clone());
    }
    let frame_w = strip.width / count;
    if frame_w == 0 {
        return None;
    }
    let mut rgba = Vec::with_capacity(frame_w * strip.height * 4);
    for y in 0..strip.height {
        let start = (y * strip.width + index * frame_w) * 4;
        rgba.extend_from_slice(strip.rgba.get(start..start + frame_w * 4)?);
    }
    Some(IconPixels { width: frame_w, height: strip.height, rgba })
}

/// Читает иконку из папки игры. Нет файла, сжатый или битый `.dds` — `None`.
pub fn load_icon_pixels(root: &Path, key: IconKey) -> Option<IconPixels> {
    let (file, frame) = icon_source(key);
    let bytes = std::fs::read(root.join(file)).ok()?;
    crop_frame(&decode_dds(&bytes)?, frame)
}

/// Ленивый кэш иконок игры в контексте egui. Иконки грузятся при первом показе; отсутствующая запоминается как
/// `None`, и интерфейс рисует текстовую замену.
#[derive(Resource, Default)]
pub struct GameIcons {
    root: std::path::PathBuf,
    cache: BTreeMap<IconKey, Option<egui::TextureHandle>>,
}

impl GameIcons {
    pub fn new(root: std::path::PathBuf) -> Self {
        Self { root, cache: BTreeMap::new() }
    }

    /// Текстура иконки (с загрузкой при первом запросе) или `None`, если файла нет.
    pub fn texture(&mut self, ctx: &egui::Context, key: IconKey) -> Option<egui::TextureId> {
        if !self.cache.contains_key(&key) {
            let handle = load_icon_pixels(&self.root, key).map(|p| {
                let image = egui::ColorImage::from_rgba_unmultiplied([p.width, p.height], &p.rgba);
                ctx.load_texture(format!("icon_{key:?}"), image, egui::TextureOptions::LINEAR)
            });
            self.cache.insert(key, handle);
        }
        self.cache.get(&key).and_then(|h| h.as_ref()).map(|h| h.id())
    }

    /// Рисует иконку 18x18; без файла — текстовая замена `fallback`.
    pub fn show(&mut self, ui: &mut egui::Ui, key: IconKey, fallback: &str) {
        match self.texture(ui.ctx(), key) {
            Some(id) => {
                ui.add(egui::Image::new(egui::load::SizedTexture::new(id, [18.0, 18.0])));
            }
            None => {
                ui.label(fallback);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn cached(&self) -> usize {
        self.cache.len()
    }
}

// ---------------------------------------------------------------------------
// Раскладка окон
// ---------------------------------------------------------------------------

/// Зазор между окнами правой колонки, пиксели.
const COLUMN_GAP: f32 = 8.0;
/// Высота заголовка сложенного окна, пока настоящая неизвестна.
const COLLAPSED_HEIGHT: f32 = 30.0;
/// Верх правой колонки: под часами.
pub const COLUMN_TOP: f32 = 52.0;

fn height_id(key: &str) -> egui::Id {
    egui::Id::new(("panel_height", key))
}

/// Запоминает высоту окна (для раскладки колонки в следующем кадре).
pub fn remember_height(ctx: &egui::Context, key: &str, height: f32) {
    ctx.data_mut(|d| d.insert_temp(height_id(key), height));
}

/// Смещение сверху для окна правой колонки: под всеми окнами из `above` с их текущей высотой, так что
/// раскрытое окно сдвигает нижние, а сложенные занимают по одной строке заголовка.
pub fn column_offset(ctx: &egui::Context, above: &[&str]) -> f32 {
    let heights: f32 = above
        .iter()
        .map(|key| ctx.data(|d| d.get_temp::<f32>(height_id(key))).unwrap_or(COLLAPSED_HEIGHT))
        .map(|h| h + COLUMN_GAP)
        .sum();
    COLUMN_TOP + heights
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
        app.init_resource::<CountryNames>()
            .init_resource::<MapFontRes>()
            .init_resource::<GameIcons>()
            .add_systems(Update, (apply_theme, country_labels_ui));
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

    fn loc(pairs: &[(&str, &str)], parties: &[(&str, &str)]) -> CountryNames {
        CountryNames {
            names: pairs.iter().map(|&(k, v)| (k.to_string(), v.to_string())).collect(),
            parties: parties.iter().map(|&(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn ideology_decides_the_historical_name() {
        let names = loc(
            &[
                ("GER", "Германия"),
                ("GER_neutrality", "Германская империя"),
                ("GER_fascism", "Германский рейх"),
                ("SOV", "Россия"),
                ("SOV_communism", "Советский Союз"),
                ("ENG", "Великобритания"),
                ("ENG_democratic", "Соединённое Королевство"),
                ("ITA", "Италия"),
                ("ITA_neutrality", "Королевство Италия"),
            ],
            &[("GER", "fascism"), ("SOV", "communism"), ("ENG", "democratic"), ("ITA", "fascism")],
        );
        assert_eq!(names.title("GER"), "Германский рейх", "<TAG>_<ruling_party>");
        assert_eq!(names.title("SOV"), "Советский Союз");
        assert_eq!(names.title("ENG"), "Соединённое Королевство");
        assert_eq!(names.title("ITA"), "Королевство Италия", "нет ITA_fascism -> <TAG>_neutrality");
        assert_eq!(label_text("GER", &names), "ГЕРМАНСКИЙ РЕЙХ");
        assert_eq!(names.header_text("SOV"), "[SOV] Советский Союз");
        // нет партии: нейтральное название, затем общее, затем тег
        let bare = loc(&[("FRA", "Франция"), ("POL", "Польша"), ("POL_neutrality", "Республика Польша")], &[]);
        assert_eq!((bare.title("FRA"), bare.title("POL"), bare.title("XXX")), ("Франция".into(), "Республика Польша".into(), "XXX".into()));
    }

    #[test]
    fn country_keys_are_filtered() {
        for key in ["GER", "GER_fascism", "SOV_communism", "D01", "GER_a_b"] {
            assert!(is_country_key(key), "{key}");
        }
        for key in ["GER_DEF", "GER_fascism_ADJ", "ger", "GERM", "GE", "", "GER_", "GER-x"] {
            assert!(!is_country_key(key), "{key}");
        }
    }

    #[test]
    fn real_localisation_uses_ruling_parties() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let names = load_country_names(root);
        // реальные названия 1936 из файлов игры (у GER фашизм — «Третий рейх», у SOV — «Советский Союз»)
        assert_eq!(names.parties.get("GER").map(String::as_str), Some("fascism"));
        assert_eq!(names.title("GER"), "Третий рейх");
        assert_eq!(names.title("SOV"), "Советский Союз");
        assert_eq!(names.title("ENG"), "Соединённое Королевство");
        assert_eq!(label_text("SOV", &names), "СОВЕТСКИЙ СОЮЗ");
    }

    /// Британия: метрополия (10 провинций со столицей) и Африка (30 провинций, больше метрополии).
    fn british_scene() -> (ProvinceControllers, ProvinceAdjacency, HashMap<u32, Vec2>, BTreeMap<String, u32>) {
        let mut controllers = BTreeMap::new();
        let mut centers = HashMap::new();
        let mut pairs = Vec::new();
        for i in 0..10u32 {
            controllers.insert(i, "ENG".to_string());
            centers.insert(i, Vec2::new(i as f32 * 10.0, 500.0));
            if i > 0 {
                pairs.push((i - 1, i));
            }
        }
        for i in 100..130u32 {
            controllers.insert(i, "ENG".to_string());
            centers.insert(i, Vec2::new(1000.0 + (i - 100) as f32 * 10.0, -900.0));
            if i > 100 {
                pairs.push((i - 1, i));
            }
        }
        // маленький остров (3 провинции) подписи не получает
        for i in 200..203u32 {
            controllers.insert(i, "ENG".to_string());
            centers.insert(i, Vec2::new(-400.0, 100.0));
            if i > 200 {
                pairs.push((i - 1, i));
            }
        }
        (ProvinceControllers(controllers), adjacency(&pairs), centers, BTreeMap::from([("ENG".to_string(), 3)]))
    }

    #[test]
    fn capital_cluster_wins_over_a_bigger_african_cluster_and_the_colony_gets_a_second_label() {
        let (controllers, adj, centers, capitals) = british_scene();
        let names = loc(&[("ENG", "Великобритания")], &[]);
        let labels = compute_labels(&controllers, &adj, &centers, &names, &capitals, None, &|_| 1.0, &|_| None);
        assert_eq!(labels.len(), 2, "метрополия + крупная колония; остров из 3 провинций без подписи");
        let main = &labels[0];
        assert!(!main.secondary);
        assert_eq!((main.provinces, main.center.y), (10, 500.0), "главная надпись над Лондоном, а не в Африке");
        assert_eq!(main.text, "ВЕЛИКОБРИТАНИЯ");
        let colony = &labels[1];
        assert!(colony.secondary && colony.provinces == 30 && colony.center.y == -900.0);
        // кегль колонии — 80% от подобранного для её территории; слово влезает в 0.75 L
        let metrics = TextMetrics::of(None, &colony.text);
        let fitted = fit_glyph_size(&colony.axis, colony.ridge.length(), &metrics);
        assert!((colony.glyph_size - fitted * 0.8).abs() < 1e-3);
        assert!((main.glyph_size - fit_glyph_size(&main.axis, main.ridge.length(), &metrics)).abs() < 1e-3);
        assert!(metrics.width_em * main.glyph_size <= 0.75 * main.axis.length + 1e-3);
    }

    #[test]
    fn without_a_controlled_capital_the_largest_cluster_is_the_main_label() {
        let (mut controllers, adj, centers, capitals) = british_scene();
        let names = CountryNames::default();
        // столица потеряна: провинция 3 под контролем другой страны
        controllers.0.insert(3, "GER".to_string());
        let labels = compute_labels(&controllers, &adj, &centers, &names, &capitals, None, &|_| 1.0, &|_| None);
        let eng: Vec<&CountryLabel> = labels.iter().filter(|l| l.tag == "ENG").collect();
        assert!(!eng[0].secondary && eng[0].provinces == 30, "главная — крупнейший кусок");
        // нет данных о столице вовсе — то же правило
        let none = compute_labels(&controllers, &adj, &centers, &names, &BTreeMap::new(), None, &|_| 1.0, &|_| None);
        assert_eq!(none.iter().find(|l| l.tag == "ENG").map(|l| l.provinces), Some(30));
        let ger = labels.iter().find(|l| l.tag == "GER").expect("GER");
        assert_eq!((ger.text.as_str(), ger.provinces), ("GER", 1), "одиночная провинция, без названия — тег");
        let lost = ProvinceControllers([(77, "ITA".to_string())].into());
        assert!(compute_labels(&lost, &adj, &centers, &names, &capitals, None, &|_| 1.0, &|_| None).is_empty(), "без центров нет надписи");
    }

    #[test]
    fn components_and_ties() {
        let adj = adjacency(&[(1, 2), (3, 4)]);
        let all: BTreeSet<u32> = [1, 2, 3, 4].into();
        assert_eq!(largest_cluster(&all, &adj.neighbors), BTreeSet::from([1, 2]), "ничья — меньший id");
        let adj = adjacency(&[(1, 2), (3, 4), (4, 5)]);
        let bigger: BTreeSet<u32> = [1, 2, 3, 4, 5].into();
        assert_eq!(largest_cluster(&bigger, &adj.neighbors), BTreeSet::from([3, 4, 5]));
        assert_eq!(components(&bigger, &adj.neighbors).len(), 2);
        assert!(largest_cluster(&BTreeSet::new(), &adj.neighbors).is_empty());
        assert!(cluster_geometry(&BTreeSet::from([1]), &HashMap::new()).is_none());
    }

    #[test]
    fn label_colors_are_charcoal_body_and_soft_light_halo() {
        assert_eq!(LABEL_INK, [22, 24, 28]);
        assert_eq!(LABEL_HALO, [235, 225, 200]);
        assert_eq!(LABEL_HALO_ALPHA, 0.45);
        let ink = egui::Color32::from_rgba_unmultiplied(LABEL_INK[0], LABEL_INK[1], LABEL_INK[2], 180);
        assert_eq!(ink.to_srgba_unmultiplied()[..3], [22, 24, 28]);
    }

    /// Полоса из трёх рядов провинций (y = -30, 0, 30) длиной 400: надпись в кегле по высоте упирается в границы.
    fn wide_strip() -> (ProvinceControllers, ProvinceAdjacency, HashMap<u32, Vec2>) {
        let mut controllers = BTreeMap::new();
        let mut centers = HashMap::new();
        let mut pairs = Vec::new();
        for i in 0..40u32 {
            for (row, y) in [-30.0_f32, 0.0, 30.0].into_iter().enumerate() {
                let id = i * 3 + row as u32;
                controllers.insert(id, "AAA".to_string());
                centers.insert(id, Vec2::new(i as f32 * 10.0, y));
                if i > 0 {
                    pairs.push((id - 3, id));
                }
                if row > 0 {
                    pairs.push((id - 1, id));
                }
            }
        }
        (ProvinceControllers(controllers), adjacency(&pairs), centers)
    }

    const FONT_H_I: &str = "common lineHeight=84 base=70 scaleW=2048 scaleH=1024\nchar id=72 x=10 y=20 width=40 height=55 xoffset=2 yoffset=17 xadvance=50\nchar id=73 x=60 y=20 width=10 height=55 xoffset=1 yoffset=17 xadvance=14\n";

    #[test]
    fn edge_letters_are_kept_inside_the_country_by_shrinking_the_font() {
        let (controllers, adj, centers) = wide_strip();
        let font = MapFont {
            font: crate::fonts::parse_bmfont(FONT_H_I).expect("шрифт"),
            width: 2048,
            height: 1024,
            coverage: Vec::new(),
            halo: Vec::new(),
        };
        let names = loc(&[("AAA", "hihihi")], &[]);
        let size_with = |land: &dyn Fn(Vec2) -> Option<u32>| {
            let labels = compute_labels(&controllers, &adj, &centers, &names, &BTreeMap::new(), Some(&font), &|_| 1.0, land);
            labels.first().map(|l| (l.glyph_size, l.ridge.clone(), l.text.clone()))
        };
        // вся полоса — суша: кегль остаётся подобранным по 0.65 W
        let (free, _, _) = size_with(&|w: Vec2| (w.y.abs() <= 60.0).then_some(0)).expect("надпись");
        // суша только в центральной полосе |y| <= 20: буквы высотой ~55 вылезали бы в «океан» — кегль уменьшается
        let narrow = |w: Vec2| (w.y.abs() <= 20.0 && w.x >= -50.0 && w.x <= 450.0).then_some(0);
        let (shrunk, ridge, text) = size_with(&narrow).expect("надпись");
        assert!(shrunk < free, "{shrunk} < {free}");
        assert!(shrunk >= free * 0.9_f32.powi(5) - 1e-3, "не более 5 шагов по 10%");
        // после уменьшения первая, средняя и последняя буквы лежат внутри страны
        let quads = layout_on_ridge(&font.font, &text, &ridge, shrunk);
        assert!(letters_inside(&quads, "AAA", &controllers, &narrow));
        let before = layout_on_ridge(&font.font, &text, &ridge, free);
        assert!(!letters_inside(&before, "AAA", &controllers, &narrow), "исходный кегль выходил за границу");
        // чужая страна на месте провинции — тоже «вне» страны
        let foreign = ProvinceControllers([(0, "BBB".to_string())].into());
        assert!(!letters_inside(&quads, "AAA", &foreign, &narrow));
        assert!(letters_inside(&[], "AAA", &controllers, &narrow), "нет букв — нечему вылезать");
    }

    #[test]
    fn glyph_mesh_maps_world_to_screen_with_the_camera_scale() {
        let quad = GlyphQuad {
            corners: [Vec2::new(0.0, 10.0), Vec2::new(20.0, 10.0), Vec2::new(20.0, 0.0), Vec2::new(0.0, 0.0)],
            uv: [Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)],
        };
        let mut near = egui::Mesh::default();
        let mut far = egui::Mesh::default();
        glyph_mesh(&[quad], Vec2::new(100.0, 100.0), 0.5, egui::TextureId::default(), egui::Color32::WHITE, &mut near);
        glyph_mesh(&[quad], Vec2::new(100.0, 100.0), 2.0, egui::TextureId::default(), egui::Color32::WHITE, &mut far);
        let width = |m: &egui::Mesh| m.vertices[1].pos.x - m.vertices[0].pos.x;
        assert_eq!((width(&near), width(&far)), (40.0, 10.0), "ближе — буквы крупнее на экране");
        assert!(near.vertices[0].pos.y < near.vertices[3].pos.y, "верх буквы выше на экране (Y вниз)");
        assert_eq!(near.indices.len(), 6);
    }

    fn dds_32(width: u32, height: u32, pixels: &[[u8; 4]]) -> Vec<u8> {
        // несжатый BGRA8: маски R=0x00FF0000, G=0x0000FF00, B=0x000000FF, A=0xFF000000
        let mut b = vec![0u8; 128];
        b[..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&height.to_le_bytes());
        b[16..20].copy_from_slice(&width.to_le_bytes());
        b[76..80].copy_from_slice(&32u32.to_le_bytes());
        b[80..84].copy_from_slice(&0x41u32.to_le_bytes());
        b[88..92].copy_from_slice(&32u32.to_le_bytes());
        for (at, mask) in [(92, 0x00FF_0000u32), (96, 0x0000_FF00), (100, 0x0000_00FF), (104, 0xFF00_0000)] {
            b[at..at + 4].copy_from_slice(&mask.to_le_bytes());
        }
        for [r, g, bl, a] in pixels {
            b.extend_from_slice(&[*bl, *g, *r, *a]);
        }
        b
    }

    #[test]
    fn dds_is_decoded_and_strips_are_cropped() {
        let bytes = dds_32(4, 1, &[[1, 2, 3, 255], [4, 5, 6, 128], [7, 8, 9, 0], [10, 11, 12, 255]]);
        let image = decode_dds(&bytes).expect("dds");
        assert_eq!((image.width, image.height), (4, 1));
        assert_eq!(&image.rgba[..8], &[1, 2, 3, 255, 4, 5, 6, 128]);
        let second = crop_frame(&image, (1, 2)).expect("кадр");
        assert_eq!((second.width, second.rgba[..4].to_vec()), (2, vec![7, 8, 9, 0]));
        assert!(crop_frame(&image, (2, 2)).is_none(), "кадра нет");
        assert!(crop_frame(&image, (0, 0)).is_none());
        // мусор, сжатый DXT и обрезанный файл — не паника
        assert!(decode_dds(b"").is_none() && decode_dds(b"DDS garbage").is_none());
        let mut dxt = bytes.clone();
        dxt[80..84].copy_from_slice(&0x4u32.to_le_bytes());
        dxt[84..88].copy_from_slice(b"DXT5");
        assert!(decode_dds(&dxt).is_none());
        assert!(decode_dds(&bytes[..bytes.len() - 4]).is_none());
    }

    #[test]
    fn missing_game_folder_gives_no_icons_without_panic() {
        let root = Path::new("Z:\\нет\\игры");
        for key in [IconKey::CivFactory, IconKey::Manpower, IconKey::Resource(crate::economy::ResourceType::Steel)] {
            assert!(load_icon_pixels(root, key).is_none());
        }
        let ctx = egui::Context::default();
        let mut icons = GameIcons::new(root.to_path_buf());
        assert!(icons.texture(&ctx, IconKey::MilFactory).is_none());
        assert!(icons.texture(&ctx, IconKey::MilFactory).is_none());
        assert_eq!(icons.cached(), 1, "отсутствие иконки тоже кэшируется");
        let mut shown = false;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                icons.show(ui, IconKey::CivFactory, "🏭");
                shown = true;
            });
        });
        assert!(shown);
    }

    #[test]
    fn real_icons_are_decoded_from_the_game() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        use crate::economy::ResourceType as R;
        let civ = load_icon_pixels(root, IconKey::CivFactory).expect("civ_factory");
        assert_eq!((civ.width, civ.height), (18, 18));
        assert!(civ.rgba.chunks(4).any(|p| p[3] > 0));
        assert!(load_icon_pixels(root, IconKey::MilFactory).is_some() && load_icon_pixels(root, IconKey::Manpower).is_some());
        let steel = load_icon_pixels(root, IconKey::Resource(R::Steel)).expect("сталь");
        let oil = load_icon_pixels(root, IconKey::Resource(R::Oil)).expect("нефть");
        assert_eq!((steel.width, steel.height), (27, 27), "кадр полосы 189x27 / 7");
        assert_ne!(steel.rgba, oil.rgba, "разные кадры");
        for r in R::ALL {
            assert!(load_icon_pixels(root, IconKey::Resource(r)).is_some(), "{r:?}");
        }
    }

    #[test]
    fn column_stacks_below_the_windows_above() {
        let ctx = egui::Context::default();
        assert_eq!(column_offset(&ctx, &[]), COLUMN_TOP);
        assert_eq!(column_offset(&ctx, &["a"]), COLUMN_TOP + COLLAPSED_HEIGHT + COLUMN_GAP, "высота неизвестна — сложенное окно");
        remember_height(&ctx, "a", 200.0);
        remember_height(&ctx, "b", 40.0);
        assert_eq!(column_offset(&ctx, &["a", "b"]), COLUMN_TOP + 200.0 + 40.0 + 2.0 * COLUMN_GAP);
    }

    #[test]
    fn unknown_country_gets_a_fallback_flag_without_panic() {
        let ctx = egui::Context::default();
        let mut flags = FlagTextures::new(PathBuf::from("Z:\\нет\\такой\\папки"));
        let colors = CountryColors::default();
        let mut shown = false;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                country_header(ui, &mut flags, &colors, &CountryNames::default(), "ZZZ", "Неизвестная");
                readiness_bars(ui, (30, 60), (0, 0));
                shown = true;
            });
        });
        assert!(shown);
        assert_eq!(flags.cached(), 1, "запасная плашка закэширована");
    }
}
