//! Карта провинций: загрузка данных HOI4, парсинг стейтов, выбор провинции курсором.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::prelude::*;
use bevy::asset::load_internal_asset;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{AsBindGroup, Extent3d, Shader, ShaderRef, TextureDimension, TextureFormat};
use bevy::sprite::{Material2d, Material2dPlugin, MaterialMesh2dBundle, Mesh2dHandle};
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts};
use image::RgbaImage;

use crate::camera::{CameraSet, MainCamera};
use crate::colors::CountryColors;
use crate::WINDOW_TITLE;

// ---------------------------------------------------------------------------
// Ресурсы и типы
// ---------------------------------------------------------------------------

/// Данные одной провинции из map/definition.csv.
#[derive(Debug, Clone)]
pub struct ProvinceInfo {
    pub id: u32,
    pub terrain: String,
}

/// Сколько пикселей границы должны касаться реки, чтобы пара провинций считалась разделённой рекой.
/// Реки HOI4 идут вдоль границ, а одиночное касание — это река, лишь пересекающая границу. Замер на игре 1936:
/// >=1 пикс. — 9012 границ, >=2 — 6710, >=3 — 5461, >=5 — 4744, >=10 — 3242; порог 3 отсекает случайные касания.
const MIN_RIVER_CONTACT_PIXELS: u32 = 3;

/// Карта провинций в оперативной памяти: пиксели + таблицы lookup.
/// Только чтение после старта, поэтому детерминизм не страдает
/// (HashMap используется исключительно для lookup, без итерации).
#[derive(Resource)]
pub struct ProvinceMap {
    image: RgbaImage,
    by_color: HashMap<[u8; 3], ProvinceInfo>,
    /// id провинции -> местность (для вывода по id).
    terrain_by_id: HashMap<u32, String>,
    /// Маска речных пикселей из `map/rivers.bmp` (тот же размер, что и карта провинций); `None`, если файла нет.
    river_mask: Option<Vec<bool>>,
    /// Карта высот из `map/heightmap.bmp` (только для шейдера рельефа); `None` — карта плоская.
    heightmap: Option<HeightMap>,
}

/// Карта высот: 8 бит на пиксель, построчно сверху вниз. Чисто визуальные данные, симуляцию не трогают.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightMap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Декодирует `heightmap.bmp` (градации серого 8 бит; 16-битный источник приводится к 8 битам).
/// Битый файл или нулевой размер — `None`, карта рисуется плоской.
pub fn decode_heightmap(bytes: &[u8]) -> Option<HeightMap> {
    let gray = image::load_from_memory(bytes).ok()?.to_luma8();
    let (width, height) = (gray.width(), gray.height());
    if width == 0 || height == 0 {
        return None;
    }
    Some(HeightMap { width, height, pixels: gray.into_raw() })
}

/// Текстура высот для шейдера: настоящая карта (R8Unorm) или плоская заглушка 1x1 — по ней шейдер
/// (размер текстуры 1x1) отключает рельеф и рисует обычную политическую карту.
pub fn height_image(heightmap: Option<&HeightMap>) -> Image {
    let (width, height, data) = match heightmap {
        Some(h) => (h.width, h.height, h.pixels.clone()),
        None => (1, 1, vec![0u8]),
    };
    Image::new(
        Extent3d { width, height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// Пары смежных провинций (меньший id, больший id), между которыми проходит река:
/// атака через такую границу — форсирование реки. Строится вместе с графом смежности.
#[derive(Resource, Default, Debug)]
pub struct ProvinceRivers(pub BTreeSet<(u32, u32)>);

impl ProvinceRivers {
    /// Есть ли река между провинциями `a` и `b`.
    pub fn crosses(&self, a: u32, b: u32) -> bool {
        self.0.contains(&(a.min(b), a.max(b)))
    }
}

impl ProvinceMap {
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    pub fn height(&self) -> u32 {
        self.image.height()
    }

    /// Мировые координаты (центр спрайта = (0,0), Y вверх) -> пиксель текстуры
    /// (левый верхний угол = (0,0), Y вниз). `None`, если вне карты.
    pub fn world_to_pixel(&self, world: Vec2) -> Option<(u32, u32)> {
        let w = self.width() as f32;
        let h = self.height() as f32;
        if !world.x.is_finite() || !world.y.is_finite() {
            return None;
        }
        let fx = (world.x + w * 0.5).floor();
        let fy = (h * 0.5 - world.y).floor(); // инверсия оси Y
        if fx < 0.0 || fy < 0.0 || fx >= w || fy >= h {
            return None;
        }
        Some((fx as u32, fy as u32))
    }

    pub fn province_at_pixel(&self, x: u32, y: u32) -> Option<&ProvinceInfo> {
        let px = self.image.get_pixel_checked(x, y)?;
        self.by_color.get(&[px[0], px[1], px[2]])
    }

    /// Карта для тестов с заданной местностью провинций: (id, terrain); остальные считаются сушей.
    #[cfg(test)]
    pub fn with_terrain_for_test(terrain: &[(u32, &str)]) -> Self {
        let terrain_by_id = terrain.iter().map(|&(id, t)| (id, t.to_string())).collect();
        Self { image: RgbaImage::new(1, 1), by_color: HashMap::new(), terrain_by_id, river_mask: None, heightmap: None }
    }

    /// Пустая карта для модульных тестов: вся суша, цветов и местности нет.
    #[cfg(test)]
    pub fn empty_for_test() -> Self {
        Self { image: RgbaImage::new(1, 1), by_color: HashMap::new(), terrain_by_id: HashMap::new(), river_mask: None, heightmap: None }
    }

    pub fn terrain_of(&self, province_id: u32) -> Option<&str> {
        self.terrain_by_id.get(&province_id).map(String::as_str)
    }

    /// Геометрический центр каждой провинции (среднее координат её пикселей)
    /// в мировых координатах (центр карты = (0,0), Y вверх).
    pub fn compute_centers(&self) -> ProvinceCenters {
        let w = self.width() as usize;
        let h = self.height() as f64;
        let half_w = w as f64 * 0.5;

        // Цвет -> слот аккумулятора. Порядок обхода by_color на результат не влияет.
        let mut slot_of: HashMap<[u8; 3], usize> = HashMap::with_capacity(self.by_color.len());
        let mut ids: Vec<u32> = Vec::with_capacity(self.by_color.len());
        for (rgb, info) in &self.by_color {
            slot_of.insert(*rgb, ids.len());
            ids.push(info.id);
        }
        let mut sums = vec![(0u64, 0u64, 0u64); ids.len()]; // (sum_x, sum_y, count)

        let mut last_key = [0u8; 3];
        let mut last_slot: Option<usize> = None;
        let mut have_last = false;
        for (py, row) in self.image.as_raw().chunks_exact(w * 4).enumerate() {
            for (px, p) in row.chunks_exact(4).enumerate() {
                let key = [p[0], p[1], p[2]];
                if !have_last || key != last_key {
                    last_slot = slot_of.get(&key).copied();
                    last_key = key;
                    have_last = true;
                }
                if let Some(s) = last_slot {
                    let acc = &mut sums[s];
                    acc.0 += px as u64;
                    acc.1 += py as u64;
                    acc.2 += 1;
                }
            }
        }

        let mut centers = HashMap::with_capacity(ids.len());
        for (id, (sx, sy, n)) in ids.into_iter().zip(sums) {
            if n == 0 {
                continue;
            }
            let mean_x = sx as f64 / n as f64 + 0.5;
            let mean_y = sy as f64 / n as f64 + 0.5;
            centers.insert(id, Vec2::new((mean_x - half_w) as f32, (h * 0.5 - mean_y) as f32));
        }
        ProvinceCenters { centers }
    }
}

/// Центры провинций в мировых координатах (только lookup, без итерации).
#[derive(Resource, Default)]
pub struct ProvinceCenters {
    pub centers: HashMap<u32, Vec2>,
}

/// Последняя провинция под курсором (для вывода только при смене).
#[derive(Resource, Default)]
pub struct HoveredProvince {
    pub id: Option<u32>,
}

/// Выбранная игроком провинция (меняется только через PlayerCommand).
#[derive(Resource, Default)]
pub struct SelectedProvince {
    pub id: Option<u32>,
}

/// Команды игрока — единая точка входа для будущего мультиплеера.
#[derive(Event, Debug, Clone, Copy)]
pub enum PlayerCommand {
    SelectProvince(u32),
}

/// Кто фактически контролирует провинцию: id провинции -> тег страны.
/// На старте совпадает с владельцем стейта; меняется при захвате (`ProvinceControllerChanged`).
#[derive(Resource, Default)]
pub struct ProvinceControllers(pub BTreeMap<u32, String>);

/// Контроль над провинцией перешёл к другой стране.
#[derive(Event, Debug, Clone)]
pub struct ProvinceControllerChanged {
    pub province_id: u32,
    pub new_controller: String,
}

/// Узлы снабжения из `map/supply_nodes.txt` (провинции-хабы).
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct SupplyNodes(pub BTreeSet<u32>);

/// Граф железных дорог из `map/railways.txt`: двусторонние связи соседних точек цепочек.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct RailwayGraph(pub BTreeMap<u32, BTreeSet<u32>>);

/// Строки без комментариев (`# ...`) как списки чисел; строка с нечисловым токеном пропускается целиком.
fn numeric_lines(text: &str) -> impl Iterator<Item = Vec<u32>> + '_ {
    text.trim_start_matches('\u{feff}').lines().filter_map(|line| {
        let line = line.split('#').next().unwrap_or("");
        let numbers: Option<Vec<u32>> = line.split_whitespace().map(|t| t.parse::<u32>().ok()).collect();
        numbers.filter(|n| !n.is_empty())
    })
}

/// `supply_nodes.txt`: пары `<уровень> <id провинции>`; уникальные id провинций (уровень >= 1, id > 0).
pub fn parse_supply_nodes(text: &str) -> SupplyNodes {
    let mut nodes = BTreeSet::new();
    for numbers in numeric_lines(text) {
        for pair in numbers.chunks_exact(2) {
            if pair[0] >= 1 && pair[1] > 0 {
                nodes.insert(pair[1]);
            }
        }
    }
    SupplyNodes(nodes)
}

/// `railways.txt`: `<уровень 1..5> <K> <id_1> ... <id_K>`; связи между соседями в цепочке в обе стороны.
/// Строки с неверным уровнем пропускаются, K сверх фактического числа id обрезается.
pub fn parse_railways(text: &str) -> RailwayGraph {
    let mut graph: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for numbers in numeric_lines(text) {
        let [level, count, ids @ ..] = numbers.as_slice() else { continue };
        if !(1..=5).contains(level) {
            continue;
        }
        let ids = &ids[..ids.len().min(*count as usize)];
        for pair in ids.windows(2) {
            if pair[0] != pair[1] {
                graph.entry(pair[0]).or_default().insert(pair[1]);
                graph.entry(pair[1]).or_default().insert(pair[0]);
            }
        }
    }
    RailwayGraph(graph)
}

/// Читает `map/supply_nodes.txt`; нет файла — пустой набор и предупреждение.
pub fn load_supply_nodes(root: &Path) -> SupplyNodes {
    match std::fs::read(root.join("map").join("supply_nodes.txt")) {
        Ok(bytes) => parse_supply_nodes(&String::from_utf8_lossy(&bytes)),
        Err(e) => {
            eprintln!("[ПРЕДУПРЕЖДЕНИЕ] supply_nodes.txt: {e}; хабы определятся по очкам победы");
            SupplyNodes::default()
        }
    }
}

/// Читает `map/railways.txt`; нет файла — пустой граф и предупреждение.
pub fn load_railways(root: &Path) -> RailwayGraph {
    match std::fs::read(root.join("map").join("railways.txt")) {
        Ok(bytes) => parse_railways(&String::from_utf8_lossy(&bytes)),
        Err(e) => {
            eprintln!("[ПРЕДУПРЕЖДЕНИЕ] railways.txt: {e}; снабжаться будут только окрестности столиц");
            RailwayGraph::default()
        }
    }
}

/// Граф смежности провинций (BTree — строгий детерминизм под мультиплеер).
#[derive(Resource, Default)]
pub struct ProvinceAdjacency {
    pub neighbors: BTreeMap<u32, BTreeSet<u32>>,
}

/// Данные стейта из history/states/*.txt
#[derive(Debug, Clone)]
pub struct StateDef {
    pub id: u32,
    pub name: String,
    pub owner: String,
    pub provinces: Vec<u32>,
    /// `victory_points = { провинция очки }` на старте: (id провинции, VP).
    pub victory_points: Vec<(u32, u32)>,
    /// `industrial_complex` — гражданские фабрики стейта.
    pub civilian_factories: u32,
    /// `arms_factory` — военные заводы стейта.
    pub military_factories: u32,
    /// `resources = { steel = N ... }` — природные ресурсы стейта (суточная добыча).
    pub resources: BTreeMap<crate::economy::ResourceType, u32>,
}

/// Все стейты мира + O(1) lookup провинция -> стейт.
#[derive(Resource, Default)]
pub struct StateRegistry {
    pub states: Vec<Arc<StateDef>>,
    pub province_to_state: HashMap<u32, Arc<StateDef>>,
}

impl StateRegistry {
    pub fn state_of(&self, province_id: u32) -> Option<&Arc<StateDef>> {
        self.province_to_state.get(&province_id)
    }

    /// Суммы фабрик по странам-владельцам: тег -> (гражданские, военные).
    /// `BTreeMap` — детерминированный порядок для экономики.
    pub fn factories_by_owner(&self) -> BTreeMap<String, (u32, u32)> {
        let mut totals: BTreeMap<String, (u32, u32)> = BTreeMap::new();
        for state in &self.states {
            let entry = totals.entry(state.owner.clone()).or_default();
            entry.0 = entry.0.saturating_add(state.civilian_factories);
            entry.1 = entry.1.saturating_add(state.military_factories);
        }
        totals
    }

    /// Природные ресурсы по странам-владельцам стейтов на старте: тег -> ресурс -> добыча.
    pub fn resources_by_owner(&self) -> BTreeMap<String, BTreeMap<crate::economy::ResourceType, u32>> {
        let mut totals: BTreeMap<String, BTreeMap<crate::economy::ResourceType, u32>> = BTreeMap::new();
        for state in &self.states {
            let entry = totals.entry(state.owner.clone()).or_default();
            for (&resource, &amount) in &state.resources {
                let slot = entry.entry(resource).or_insert(0);
                *slot = slot.saturating_add(amount);
            }
        }
        totals
    }
}

// ---------------------------------------------------------------------------
// Плагин
// ---------------------------------------------------------------------------

/// Загружает данные игры из `root` и регистрирует системы карты.
/// Если карту загрузить не удалось — процесс завершается с кодом 1.
pub struct MapPlugin {
    pub root: PathBuf,
}

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        let province_map = match load_province_map(&self.root) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[ОШИБКА] {e}");
                std::process::exit(1);
            }
        };

        let registry = match load_all_states(&self.root.join("history").join("states")) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[ПРЕДУПРЕЖДЕНИЕ] {e}");
                StateRegistry::default()
            }
        };

        let centers = province_map.compute_centers();
        println!("[УСПЕХ] Центров провинций вычислено: {}", centers.centers.len());

        // Order of Battle 1936 (GER, FRA): читается на лету, спавн — в division.rs
        app.insert_resource(crate::oob::load_oob(&self.root));

        // Логистика: узлы снабжения и железные дороги
        let supply_nodes = load_supply_nodes(&self.root);
        let railways = load_railways(&self.root);
        println!(
            "[СНАБЖЕНИЕ] Узлов снабжения: {}, железнодорожных точек: {}",
            supply_nodes.0.len(),
            railways.0.len()
        );
        app.insert_resource(supply_nodes).insert_resource(railways);

        // Аутентичные цвета стран из common/countries/colors.txt и файлов стран
        app.insert_resource(crate::colors::load_country_colors(&self.root));

        // Стартовые контроллеры = владельцы стейтов
        let controllers = ProvinceControllers(
            registry.province_to_state.iter().map(|(&province, state)| (province, state.owner.clone())).collect(),
        );

        // Шейдер карты вшит в бинарник; материал рисует карту по палитре контроллеров
        load_internal_asset!(app, POLITICAL_SHADER, "political.wgsl", Shader::from_wgsl);
        app.add_plugins(Material2dPlugin::<PoliticalMaterial>::default())
            .add_event::<ProvinceControllerChanged>()
            .insert_resource(controllers)
            .add_systems(Update, repaint_changed_provinces);

        app.insert_resource(centers)
            .insert_resource(province_map)
            .insert_resource(registry)
            .init_resource::<HoveredProvince>()
            .init_resource::<SelectedProvince>()
            .init_resource::<ProvinceAdjacency>()
            .init_resource::<ProvinceRivers>()
            .add_event::<PlayerCommand>()
            .add_systems(Startup, setup_map)
            .add_systems(
                Update,
                (
                    // Выбор провинции читает камеру после её перемещения в этом кадре
                    pick_province.after(CameraSet),
                    click_to_command.after(pick_province),
                    apply_player_commands.after(click_to_command),
                    ui_state_panel.after(apply_player_commands),
                ),
            );
    }
}

/// Цвет воды / морских и нейтральных зон.
const WATER_COLOR: [u8; 3] = [20, 40, 70];

// ---------------------------------------------------------------------------
// Рендер политической карты на GPU: текстура id провинций + палитра контроллеров
// ---------------------------------------------------------------------------

/// Ширина палитры в текселях; должна совпадать с `PALETTE_WIDTH` в `political.wgsl`.
const PALETTE_WIDTH: usize = 256;
/// Хэндл встроенного шейдера (см. `load_internal_asset!` в `MapPlugin::build`).
const POLITICAL_SHADER: Handle<Shader> = Handle::weak_from_u128(0x7a3c_91e4_5b20_4f6d_8c17_d2e0_a94b_6f31);

/// Материал карты. Цвета, затемнение границ провинций и тёмные границы государств считает шейдер
/// по двум текстурам: id провинции на пиксель (статична) и палитра «id -> цвет контроллера».
/// Смена контроля меняет один тексель палитры, карта целиком не перерисовывается.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct PoliticalMaterial {
    #[texture(0)]
    pub ids: Handle<Image>,
    #[texture(1)]
    pub palette: Handle<Image>,
    /// Высоты суши (R8Unorm); текстура 1x1 означает «рельефа нет».
    #[texture(2)]
    #[sampler(3)]
    pub heights: Handle<Image>,
}

impl Material2d for PoliticalMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(POLITICAL_SHADER)
    }
}

/// Хэндлы ресурсов рендера карты для точечных обновлений.
#[derive(Resource)]
pub struct MapPaint {
    palette: Handle<Image>,
    material: Handle<PoliticalMaterial>,
}

/// Цвет контроллера в палитре. Альфа 255 — суша, 0 — вода / без стейта (границы не рисуются).
fn palette_texel(controller: Option<&str>, colors: &CountryColors) -> [u8; 4] {
    match controller {
        Some(tag) => {
            let [r, g, b] = colors.map_rgb(tag);
            [r, g, b, 255]
        }
        None => [WATER_COLOR[0], WATER_COLOR[1], WATER_COLOR[2], 0],
    }
}

/// Строит текстуру id провинций (R, G = младший / старший байт id) и граф смежности за один обход.
pub(crate) fn build_id_texture_and_adjacency(map: &ProvinceMap) -> (Vec<u8>, ProvinceAdjacency, ProvinceRivers) {
    let w = map.width() as usize;
    let h = map.height() as usize;
    let raw = map.image.as_raw();

    let color_to_id: HashMap<[u8; 3], u32> = map.by_color.iter().map(|(rgb, info)| (*rgb, info.id)).collect();

    let mut ids = vec![0u8; raw.len()];
    // Уникальные рёбра (меньший id, больший id); порядок вставки не влияет на результат.
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    // Рёбра, на стыке которых есть речной пиксель (по любую сторону границы)
    let mut river_contacts: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    let mut last_pair = ([0u8; 3], [0u8; 3]); // кэш: подряд идущие одинаковые стыки
    let mut last_key = [0u8; 3];
    let mut last_id = 0u32;
    let mut have_last = false;

    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let key_here = [raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2]];

            if !have_last || key_here != last_key {
                last_id = color_to_id.get(&key_here).copied().unwrap_or(0);
                last_key = key_here;
                have_last = true;
            }
            ids[i * 4] = (last_id & 0xFF) as u8;
            ids[i * 4 + 1] = ((last_id >> 8) & 0xFF) as u8;
            ids[i * 4 + 3] = 255;

            // Смежность: только горизонтальные/вертикальные стыки (правый и нижний сосед
            // покрывают все пары без дублей; диагонали не учитываются).
            for n in [(x + 1 < w).then(|| i + 1), (y + 1 < h).then(|| i + w)].into_iter().flatten() {
                let key_n = [raw[n * 4], raw[n * 4 + 1], raw[n * 4 + 2]];
                if key_n == key_here {
                    continue;
                }
                // Река на границе провинций: пиксель реки по любую сторону стыка (проверка до кэша пар)
                if map.river_mask.as_ref().is_some_and(|mask| mask[i] || mask[n]) {
                    if let (Some(&a), Some(&b)) = (color_to_id.get(&key_here), color_to_id.get(&key_n)) {
                        if a != b {
                            *river_contacts.entry((a.min(b), a.max(b))).or_default() += 1;
                        }
                    }
                }
                if (key_here, key_n) == last_pair {
                    continue;
                }
                last_pair = (key_here, key_n);
                if let (Some(&a), Some(&b)) = (color_to_id.get(&key_here), color_to_id.get(&key_n)) {
                    if a != b {
                        edges.insert((a.min(b), a.max(b)));
                    }
                }
            }
        }
    }

    // Рёбра -> детерминированный граф (BTreeMap/BTreeSet).
    let mut adjacency = ProvinceAdjacency::default();
    for &(a, b) in &edges {
        adjacency.neighbors.entry(a).or_default().insert(b);
        adjacency.neighbors.entry(b).or_default().insert(a);
    }
    println!(
        "[ТОПОЛОГИЯ] Граф смежности построен. Всего узлов: {}, уникальных связей: {}",
        adjacency.neighbors.len(),
        edges.len()
    );
    let at_least = |n: u32| river_contacts.values().filter(|&&c| c >= n).count();
    println!("[РЕКИ] Границ с рекой: >=1 пикс. {}, >=2 {}, >=3 {}, >=5 {}, >=10 {}", at_least(1), at_least(2), at_least(3), at_least(5), at_least(10));
    let river_edges = river_contacts.into_iter().filter(|&(_, c)| c >= MIN_RIVER_CONTACT_PIXELS).map(|(pair, _)| pair).collect();
    (ids, adjacency, ProvinceRivers(river_edges))
}

pub fn setup_map(
    mut commands: Commands,
    map: Res<ProvinceMap>,
    controllers: Res<ProvinceControllers>,
    colors: Res<CountryColors>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<PoliticalMaterial>>,
) {
    let (ids, adjacency, rivers) = build_id_texture_and_adjacency(&map);
    commands.insert_resource(adjacency);
    commands.insert_resource(rivers);

    // Палитра: тексель (id % 256, id / 256) = цвет контроллера провинции.
    let max_id = map.by_color.values().map(|p| p.id).max().unwrap_or(0) as usize;
    let rows = max_id / PALETTE_WIDTH + 1;
    let mut palette = Vec::with_capacity(PALETTE_WIDTH * rows * 4);
    for _ in 0..PALETTE_WIDTH * rows {
        palette.extend_from_slice(&palette_texel(None, &colors));
    }
    for (&id, tag) in &controllers.0 {
        let off = id as usize * 4;
        if let Some(px) = palette.get_mut(off..off + 4) {
            px.copy_from_slice(&palette_texel(Some(tag), &colors));
        }
    }

    // id провинций — точные байты (без sRGB), хранится только на GPU; палитра — sRGB, как раньше карта.
    let ids_image = Image::new(
        Extent3d { width: map.width(), height: map.height(), depth_or_array_layers: 1 },
        TextureDimension::D2,
        ids,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    let palette_image = Image::new(
        Extent3d { width: PALETTE_WIDTH as u32, height: rows as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        palette,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(), // данные остаются в RAM для точечных правок
    );
    let palette_handle = images.add(palette_image);
    let heights = images.add(height_image(map.heightmap.as_ref()));
    println!(
        "[РЕЛЬЕФ] {}",
        match &map.heightmap {
            Some(h) => format!("карта высот {}x{} загружена", h.width, h.height),
            None => "карта высот недоступна, плоский режим".to_string(),
        }
    );
    let material =
        materials.add(PoliticalMaterial { ids: images.add(ids_image), palette: palette_handle.clone(), heights });
    commands.insert_resource(MapPaint { palette: palette_handle, material: material.clone() });

    // Меш-прямоугольник по центру (0,0), масштаб 1:1. Оригинальный provinces.bmp остаётся в RAM
    // (ProvinceMap) для определения провинции под курсором.
    commands.spawn(MaterialMesh2dBundle {
        mesh: Mesh2dHandle(meshes.add(Rectangle::new(map.width() as f32, map.height() as f32))),
        material,
        ..default()
    });
    println!("[УСПЕХ] Карта на GPU: {}x{}, палитра {} цветов", map.width(), map.height(), PALETTE_WIDTH * rows);
}

/// Контроль провинции сменился: правим один тексель палитры, остальные данные карты не трогаем.
fn repaint_changed_provinces(
    mut events: EventReader<ProvinceControllerChanged>,
    paint: Option<Res<MapPaint>>,
    colors: Res<CountryColors>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<PoliticalMaterial>>,
) {
    let changes: Vec<ProvinceControllerChanged> = events.read().cloned().collect();
    let Some(paint) = paint else { return };
    if changes.is_empty() {
        return;
    }
    if let Some(image) = images.get_mut(&paint.palette) {
        for change in &changes {
            let off = change.province_id as usize * 4;
            if let Some(px) = image.data.get_mut(off..off + 4) {
                px.copy_from_slice(&palette_texel(Some(&change.new_controller), &colors));
            }
        }
    }
    // В Bevy 0.14 перезагруженная текстура не пересоздаёт bind group материала сама:
    // помечаем материал изменённым, чтобы он подхватил новую палитру.
    let _ = materials.get_mut(&paint.material);
}

// ---------------------------------------------------------------------------
// Парсер стейтов
// ---------------------------------------------------------------------------

/// Разбивает текст Paradox-script на токены: слова, `{`, `}`, `=`, строки в кавычках.
/// Комментарии (`# ...`) отбрасываются. Кавычки с значения снимаются.
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for line in text.lines() {
        let mut chars = line.chars();
        let mut cur = String::new();
        let flush = |cur: &mut String, tokens: &mut Vec<String>| {
            if !cur.is_empty() {
                tokens.push(std::mem::take(cur));
            }
        };
        while let Some(c) = chars.next() {
            match c {
                '#' => break,
                '"' => {
                    flush(&mut cur, &mut tokens);
                    let mut s = String::new();
                    for c2 in chars.by_ref() {
                        if c2 == '"' {
                            break;
                        }
                        s.push(c2);
                    }
                    tokens.push(s);
                }
                '{' | '}' | '=' => {
                    flush(&mut cur, &mut tokens);
                    tokens.push(c.to_string());
                }
                c if c.is_whitespace() => flush(&mut cur, &mut tokens),
                c => cur.push(c),
            }
        }
        flush(&mut cur, &mut tokens);
    }
    tokens
}

/// Парсит содержимое файла стейта: `id`, `name`, `owner` и блок `provinces`.
fn parse_state(text: &str) -> Option<StateDef> {
    let text = text.trim_start_matches('\u{feff}'); // BOM
    let tokens = tokenize(text);

    let mut id = None;
    let mut name = None;
    let mut owner = None;
    let mut provinces = Vec::new();
    let mut civilian_factories = None;
    let mut military_factories = None;
    let mut victory_points: Vec<(u32, u32)> = Vec::new();
    let mut resources: BTreeMap<crate::economy::ResourceType, u32> = BTreeMap::new();
    let mut depth = 0usize;

    let mut i = 0;
    while i < tokens.len() {
        match tokens[i].as_str() {
            "{" => depth += 1,
            "}" => depth = depth.saturating_sub(1),
            key => {
                let is_assign = tokens.get(i + 1).map(String::as_str) == Some("=");
                if is_assign {
                    let value = tokens.get(i + 2).filter(|t| !matches!(t.as_str(), "{" | "}"));
                    match (key, depth) {
                        ("id", 1) if id.is_none() => {
                            id = value.and_then(|t| t.parse::<u32>().ok());
                        }
                        ("name", 1) if name.is_none() => {
                            name = value.cloned();
                        }
                        // owner внутри history = { ... } (depth 2); берём первое вхождение
                        ("owner", _) if owner.is_none() => {
                            owner = value.cloned();
                        }
                        // history { buildings { industrial_complex = N } }: ключ на глубине 3.
                        // Здания провинций (`123 = { naval_base = 1 }`) и датированные блоки глубже.
                        ("industrial_complex", 3) if civilian_factories.is_none() => {
                            civilian_factories = value.and_then(|t| t.parse::<u32>().ok());
                        }
                        ("arms_factory", 3) if military_factories.is_none() => {
                            military_factories = value.and_then(|t| t.parse::<u32>().ok());
                        }
                        // history { victory_points = { 6521 50 } }: ключ на глубине 2, внутри «провинция очки»
                        // (датированные блоки глубже и не учитываются — берём состояние на 1936 год)
                        ("victory_points", 2) => {
                            if tokens.get(i + 2).map(String::as_str) == Some("{") {
                                let mut j = i + 3;
                                let mut numbers = Vec::new();
                                while j < tokens.len() && tokens[j] != "}" {
                                    if let Ok(n) = tokens[j].parse::<u32>() {
                                        numbers.push(n);
                                    }
                                    j += 1;
                                }
                                if let [province, points, ..] = numbers[..] {
                                    victory_points.push((province, points));
                                }
                                i = j; // остановились на закрывающей `}`
                            }
                        }
                        // resources = { steel = 50 tungsten = 44 chromium = 25 }: ключи на глубине 2;
                        // неизвестные ресурсы (chromium) пропускаются
                        ("resources", 1) => {
                            if tokens.get(i + 2).map(String::as_str) == Some("{") {
                                let mut j = i + 3;
                                while j < tokens.len() && tokens[j] != "}" {
                                    if tokens.get(j + 1).map(String::as_str) == Some("=") {
                                        let kind = crate::economy::ResourceType::from_key(&tokens[j]);
                                        let amount = tokens.get(j + 2).and_then(|t| t.parse::<u32>().ok());
                                        if let (Some(kind), Some(amount)) = (kind, amount) {
                                            let slot = resources.entry(kind).or_insert(0);
                                            *slot = slot.saturating_add(amount);
                                        }
                                        j += 3;
                                    } else {
                                        j += 1;
                                    }
                                }
                                i = j; // остановились на закрывающей `}`
                            }
                        }
                        ("provinces", 1) if provinces.is_empty() => {
                            if tokens.get(i + 2).map(String::as_str) == Some("{") {
                                let mut j = i + 3;
                                while j < tokens.len() && tokens[j] != "}" {
                                    if let Ok(p) = tokens[j].parse::<u32>() {
                                        provinces.push(p);
                                    }
                                    j += 1;
                                }
                                i = j; // остановились на закрывающей `}`; она не меняет depth
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        i += 1;
    }

    let id = id?;
    Some(StateDef {
        id,
        name: name.unwrap_or_else(|| format!("STATE_{id}")),
        owner: owner.unwrap_or_else(|| "?".to_string()),
        provinces,
        civilian_factories: civilian_factories.unwrap_or(0),
        military_factories: military_factories.unwrap_or(0),
        victory_points,
        resources,
    })
}

/// Читает все .txt из `states_dir` (в алфавитном порядке — для детерминизма)
/// и строит реестр стейтов с O(1) лукапом провинция -> стейт.
pub fn load_all_states(states_dir: &Path) -> Result<StateRegistry, String> {
    let entries =
        std::fs::read_dir(states_dir).map_err(|e| format!("не удалось открыть {:?}: {e}", states_dir))?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("txt")))
        .collect();
    files.sort();

    if files.is_empty() {
        return Err(format!("в {:?} нет .txt файлов", states_dir));
    }

    let mut registry = StateRegistry::default();
    let mut conflicts = 0usize;

    for path in &files {
        let bytes = match read_bytes(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[ПРЕДУПРЕЖДЕНИЕ] {e}");
                continue;
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let Some(state) = parse_state(&text) else {
            eprintln!("[ПРЕДУПРЕЖДЕНИЕ] не удалось разобрать стейт {:?}", path.file_name().unwrap_or_default());
            continue;
        };

        let state = Arc::new(state);
        for &province in &state.provinces {
            // Провинция должна принадлежать одному стейту; при дубле остаётся первый.
            if registry.province_to_state.contains_key(&province) {
                conflicts += 1;
            } else {
                registry.province_to_state.insert(province, Arc::clone(&state));
            }
        }
        registry.states.push(state);
    }

    println!(
        "[УСПЕХ] Загружено стейтов: {}, привязано провинций: {}",
        registry.states.len(),
        registry.province_to_state.len()
    );
    if conflicts > 0 {
        eprintln!("[ПРЕДУПРЕЖДЕНИЕ] провинций, заявленных в нескольких стейтах: {conflicts}");
    }
    Ok(registry)
}

// ---------------------------------------------------------------------------
// Загрузка данных (единая точка чтения файлов — задел под будущую VFS)
// ---------------------------------------------------------------------------

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("не удалось прочитать {:?}: {e}", path))
}

pub(crate) fn load_province_map(root: &Path) -> Result<ProvinceMap, String> {
    // provinces.bmp -> RgbaImage в памяти
    let bmp_bytes = read_bytes(&root.join("map").join("provinces.bmp"))?;
    let image = image::load_from_memory(&bmp_bytes)
        .map_err(|e| format!("не удалось декодировать provinces.bmp: {e}"))?
        .to_rgba8();

    // definition.csv: ID;R;G;B;type;coastal;terrain;continent
    let csv_bytes = read_bytes(&root.join("map").join("definition.csv"))?;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b';')
        .has_headers(false)
        .flexible(true)
        .from_reader(csv_bytes.as_slice());

    let mut by_color = HashMap::new();
    let mut terrain_by_id = HashMap::new();
    for record in reader.byte_records() {
        let Ok(rec) = record else { continue };
        let field = |i: usize| rec.get(i).map(|b| String::from_utf8_lossy(b).trim().to_string());
        let parse_u = |i: usize| field(i).and_then(|s| s.parse::<u32>().ok());
        let parse_c = |i: usize| field(i).and_then(|s| s.parse::<u8>().ok());

        let (Some(id), Some(r), Some(g), Some(b)) =
            (parse_u(0), parse_c(1), parse_c(2), parse_c(3))
        else {
            continue; // пустые/битые строки пропускаем
        };
        let terrain = field(6).unwrap_or_else(|| "unknown".to_string());
        terrain_by_id.insert(id, terrain.clone());
        by_color.insert([r, g, b], ProvinceInfo { id, terrain });
    }

    println!(
        "[OK] provinces.bmp {}x{}, провинций в definition.csv: {}",
        image.width(),
        image.height(),
        by_color.len()
    );
    let river_mask = load_river_mask(root, image.width(), image.height());
    let heightmap = load_heightmap(root);
    Ok(ProvinceMap { image, by_color, terrain_by_id, river_mask, heightmap })
}

/// `map/heightmap.bmp` -> карта высот. Нет файла или он битый — предупреждение и плоский режим.
fn load_heightmap(root: &Path) -> Option<HeightMap> {
    let path = root.join("map").join("heightmap.bmp");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("[ПРЕДУПРЕЖДЕНИЕ] {}: {e}; рельеф отключён, карта плоская", path.display());
            return None;
        }
    };
    let heightmap = decode_heightmap(&bytes);
    if heightmap.is_none() {
        eprintln!("[ПРЕДУПРЕЖДЕНИЕ] heightmap.bmp не удалось декодировать; рельеф отключён, карта плоская");
    }
    heightmap
}

/// `map/rivers.bmp` -> маска речных пикселей. В файле реки нарисованы цветами палитры (индексы 0..=11),
/// а фоном служат серый (122,122,122) и белый (255,255,255). Нет файла или размер не совпал — рек нет.
fn load_river_mask(root: &Path, width: u32, height: u32) -> Option<Vec<bool>> {
    let bytes = std::fs::read(root.join("map").join("rivers.bmp")).ok()?;
    let rivers = image::load_from_memory(&bytes).ok()?.to_rgb8();
    if rivers.width() != width || rivers.height() != height {
        eprintln!("[ПРЕДУПРЕЖДЕНИЕ] rivers.bmp другого размера, реки не учитываются");
        return None;
    }
    Some(rivers.pixels().map(|p| p.0 != [122, 122, 122] && p.0 != [255, 255, 255]).collect())
}

// ---------------------------------------------------------------------------
// Определение провинции под курсором
// ---------------------------------------------------------------------------

pub fn pick_province(
    map: Res<ProvinceMap>,
    mut hovered: ResMut<HoveredProvince>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    let Ok(mut window) = windows.get_single_mut() else { return };
    let Ok((camera, cam_tf)) = cameras.get_single() else { return };

    // Курсор вне окна -> None; вне карты -> None
    let province = window
        .cursor_position()
        .and_then(|cursor| camera.viewport_to_world_2d(cam_tf, cursor))
        .and_then(|world| map.world_to_pixel(world))
        .and_then(|(x, y)| map.province_at_pixel(x, y));

    let new_id = province.map(|p| p.id);
    if new_id == hovered.id {
        return; // ничего не изменилось
    }
    hovered.id = new_id;

    window.title = match province {
        Some(p) => format!("{WINDOW_TITLE} — Province ID: {} | Terrain: {}", p.id, p.terrain),
        None => WINDOW_TITLE.to_string(),
    };
}

/// ЛКМ -> команда выбора провинции под курсором (состояние здесь не меняется).
fn click_to_command(
    mouse: Res<ButtonInput<MouseButton>>,
    hovered: Res<HoveredProvince>,
    mut commands_out: EventWriter<PlayerCommand>,
    mut contexts: EguiContexts,
) {
    // Клик по GUI не должен проваливаться на карту
    if contexts.try_ctx_mut().is_none_or(|ctx| ctx.wants_pointer_input()) {
        return;
    }
    if mouse.just_pressed(MouseButton::Left) {
        if let Some(id) = hovered.id {
            commands_out.send(PlayerCommand::SelectProvince(id));
        }
    }
}

/// Единственное место, где команды игрока применяются к состоянию.
fn apply_player_commands(
    mut events: EventReader<PlayerCommand>,
    mut selected: ResMut<SelectedProvince>,
    map: Res<ProvinceMap>,
    registry: Res<StateRegistry>,
) {
    for cmd in events.read() {
        match *cmd {
            PlayerCommand::SelectProvince(id) => {
                selected.id = Some(id);
                let terrain = map.terrain_of(id).unwrap_or("unknown");
                match registry.state_of(id) {
                    Some(s) => println!(
                        "[КЛИК] Провинция: {id} | Стейт: {} ({}) | Владелец: {} | Местность: {terrain}",
                        s.id, s.name, s.owner
                    ),
                    None => println!(
                        "[КЛИК] Провинция: {id} | Стейт: — (нет) | Владелец: — | Местность: {terrain}"
                    ),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// UI: инфо-панель выбранной провинции
// ---------------------------------------------------------------------------

fn ui_state_panel(
    mut contexts: EguiContexts,
    mut selected: ResMut<SelectedProvince>,
    map: Res<ProvinceMap>,
    states: Res<StateRegistry>,
    adjacency: Res<ProvinceAdjacency>,
    controllers: Res<ProvinceControllers>,
    colors: Res<CountryColors>,
    mut flags: ResMut<crate::flags::FlagTextures>,
) {
    let Some(id) = selected.id else { return };
    let Some(ctx) = contexts.try_ctx_mut() else { return };

    let terrain = map.terrain_of(id).unwrap_or("unknown");
    let state = states.state_of(id);
    let frame = egui::Frame::window(&ctx.style()).fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 200));

    let mut deselect = false;
    egui::Window::new("Данные региона")
        .anchor(egui::Align2::LEFT_BOTTOM, [10.0, -10.0])
        .resizable(false)
        .collapsible(false)
        .frame(frame)
        .show(ctx, |ui| {
            ui.label(format!("ID провинции: {id}"));
            ui.label(format!("Местность: {terrain}"));
            match state {
                Some(s) => {
                    ui.label(format!("Стейт: {} ({})", s.id, s.name));
                    // Контроллер может отличаться от владельца (оккупация)
                    let controller = controllers.0.get(&id).unwrap_or(&s.owner);
                    ui.horizontal(|ui| {
                        ui.label("Контролирует:");
                        flags.show(ui, controller, &colors);
                        ui.label(egui::RichText::new(controller).color(colors.ui_color32(controller)).strong());
                        ui.label("(Владелец:");
                        flags.show(ui, &s.owner, &colors);
                        ui.label(egui::RichText::new(&s.owner).color(colors.ui_color32(&s.owner)).strong());
                        ui.label(")");
                    });
                }
                None => {
                    ui.label("Стейт: — (нет)");
                    ui.label("Контролирует: —");
                }
            }
            ui.separator();
            let list: Vec<String> = adjacency
                .neighbors
                .get(&id)
                .map(|set| set.iter().map(u32::to_string).collect())
                .unwrap_or_default();
            ui.label(format!("Соседние провинции ({}):", list.len()));
            ui.set_max_width(260.0);
            ui.add(egui::Label::new(format!("[{}]", list.join(", "))).wrap());
            ui.separator();
            if ui.button("Снять выделение").clicked() {
                deselect = true;
            }
        });

    if deselect {
        selected.id = None;
    }
}

#[cfg(test)]
mod palette_color_tests {
    use super::*;
    use crate::colors::{CountryColor, FALLBACK_COLOR};

    fn colors() -> CountryColors {
        CountryColors(BTreeMap::from([
            ("FRA".to_string(), CountryColor { map: [57, 113, 228], ui: [74, 147, 255] }),
            ("GER".to_string(), CountryColor { map: [102, 96, 87], ui: [138, 155, 116] }),
        ]))
    }

    #[test]
    fn map_palette_uses_authentic_country_colors() {
        let colors = colors();
        // карта красится цветом `color`, не цветом интерфейса
        assert_eq!(palette_texel(Some("FRA"), &colors), [57, 113, 228, 255]);
        assert_eq!(palette_texel(Some("GER"), &colors), [102, 96, 87, 255]);
    }

    #[test]
    fn unknown_controller_is_painted_neutral_gray_and_water_stays_water() {
        let colors = colors();
        let [r, g, b, a] = palette_texel(Some("XXX"), &colors);
        assert_eq!([r, g, b], FALLBACK_COLOR);
        assert_eq!(a, 255, "неизвестная страна остаётся сушей");
        assert_eq!(palette_texel(None, &colors), [WATER_COLOR[0], WATER_COLOR[1], WATER_COLOR[2], 0]);
        // пустой ресурс не приводит к панике
        assert_eq!(palette_texel(Some("FRA"), &CountryColors::default())[..3], FALLBACK_COLOR);
    }
}

#[cfg(test)]
mod resource_parse_tests {
    use super::*;
    use crate::economy::ResourceType::*;

    const SWEDEN: &str = "state = {\n\tid = 666\n\tname = \"STATE_666\"\n\tmanpower = 100222\n\n\tresources = {\n\t\ttungsten = 44\n\t\tsteel = 50\n\t\tchromium = 25\n\t}\n\n\thistory = {\n\t\towner = SWE\n\t\tbuildings = {\n\t\t\tindustrial_complex = 2\n\t\t}\n\t}\n\tprovinces = {\n\t\t1 2 3\n\t}\n}\n";

    #[test]
    fn resources_block_is_parsed_and_unknown_kinds_are_skipped() {
        let state = parse_state(SWEDEN).expect("стейт");
        assert_eq!(state.resources.get(&Steel), Some(&50));
        assert_eq!(state.resources.get(&Tungsten), Some(&44));
        assert_eq!(state.resources.len(), 2, "chromium игнорируется");
        // остальные поля не пострадали
        assert_eq!((state.owner.as_str(), state.civilian_factories, state.provinces.len()), ("SWE", 2, 3));
    }

    #[test]
    fn compact_syntax_comments_and_missing_block() {
        let text = "state = {\n id = 1\n resources = {\n aluminium=2 # was: 4\n oil = 7 rubber=3\n steel = 24 # was: 40\n }\n history = { owner = FRA }\n provinces = { 9 }\n}";
        let state = parse_state(text).expect("стейт");
        assert_eq!(state.resources.into_iter().collect::<Vec<_>>(), vec![(Steel, 24), (Aluminium, 2), (Rubber, 3), (Oil, 7)]);
        let bare = parse_state("state = { id = 2 history = { owner = FRA } provinces = { 1 } }").expect("стейт");
        assert!(bare.resources.is_empty());
    }

    #[test]
    fn resources_are_summed_by_owner() {
        let a = parse_state(SWEDEN).expect("a");
        let b = parse_state(&SWEDEN.replace("id = 666", "id = 667").replace("steel = 50", "steel = 4")).expect("b");
        let c = parse_state(&SWEDEN.replace("id = 666", "id = 668").replace("owner = SWE", "owner = NOR")).expect("c");
        let registry = StateRegistry {
            states: vec![Arc::new(a), Arc::new(b), Arc::new(c)],
            province_to_state: HashMap::new(),
        };
        let totals = registry.resources_by_owner();
        assert_eq!(totals["SWE"][&Steel], 54);
        assert_eq!(totals["SWE"][&Tungsten], 88);
        assert_eq!(totals["NOR"][&Steel], 50);
        assert!(!totals["SWE"].contains_key(&Oil));
    }

    #[test]
    fn real_data_resources_are_read() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let registry = load_all_states(&root.join("history").join("states")).expect("стейты");
        let totals = registry.resources_by_owner();
        let steel = |tag: &str| totals.get(tag).and_then(|m| m.get(&Steel)).copied().unwrap_or(0);
        println!("сталь: GER {} SWE {} FRA {}", steel("GER"), steel("SWE"), steel("FRA"));
        assert!(steel("SWE") > 0 && steel("GER") > 0, "у Швеции и Германии есть сталь");
        // стейт 666 (Norrnorrland) — 50 стали и 44 вольфрама
        let state = registry.states.iter().find(|s| s.id == 666).expect("стейт 666");
        assert_eq!((state.resources.get(&Steel), state.resources.get(&Tungsten)), (Some(&50), Some(&44)));
    }
}

#[cfg(test)]
mod relief_tests {
    use super::*;

    fn gray_bmp(width: u32, height: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
        let img = image::GrayImage::from_fn(width, height, |x, y| image::Luma([f(x, y)]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Bmp).expect("запись BMP");
        out.into_inner()
    }

    #[test]
    fn grayscale_bmp_is_decoded_to_heights() {
        let bytes = gray_bmp(8, 4, |x, y| (x * 10 + y) as u8);
        let h = decode_heightmap(&bytes).expect("карта высот");
        assert_eq!((h.width, h.height, h.pixels.len()), (8, 4, 32));
        assert_eq!(h.pixels[0], 0);
        assert_eq!(h.pixels[7], 70, "строка 0, x = 7");
        assert_eq!(h.pixels[8 + 2], 21, "строка 1, x = 2");
    }

    #[test]
    fn broken_heightmap_gives_none_without_panic() {
        assert!(decode_heightmap(b"").is_none());
        assert!(decode_heightmap(b"BM garbage").is_none());
        let mut truncated = gray_bmp(8, 8, |_, _| 5);
        truncated.truncate(30);
        assert!(decode_heightmap(&truncated).is_none());
    }

    #[test]
    fn missing_file_means_flat_mode() {
        assert!(load_heightmap(Path::new("Z:\\нет\\такой\\папки")).is_none());
        let flat = height_image(None);
        assert_eq!((flat.width(), flat.height()), (1, 1), "1x1 — шейдер рисует карту плоской");
        assert_eq!(flat.texture_descriptor.format, TextureFormat::R8Unorm);
    }

    #[test]
    fn height_image_is_r8_with_the_source_size() {
        let h = HeightMap { width: 3, height: 2, pixels: vec![1, 2, 3, 4, 5, 6] };
        let image = height_image(Some(&h));
        assert_eq!((image.width(), image.height()), (3, 2));
        assert_eq!(image.texture_descriptor.format, TextureFormat::R8Unorm);
    }

    #[test]
    fn real_heightmap_matches_the_province_map() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let h = load_heightmap(root).expect("heightmap.bmp");
        let map = load_province_map(root).expect("карта");
        assert_eq!((h.width, h.height), (map.width(), map.height()));
        assert_eq!(h.pixels.len(), (h.width * h.height) as usize);
        let (min, max) = (h.pixels.iter().min().copied().unwrap_or(0), h.pixels.iter().max().copied().unwrap_or(0));
        assert!(max > min + 50, "в рельефе есть перепад высот: {min}..{max}");
    }
}

#[cfg(test)]
mod supply_parse_tests {
    use super::*;

    #[test]
    fn supply_nodes_are_unique_province_ids() {
        let nodes = parse_supply_nodes("1 67 \n1 101\n# комментарий\n2 67\n\n1 abc\n0 55\n1 121 # хвост\n1\n");
        assert_eq!(nodes.0.into_iter().collect::<Vec<_>>(), vec![67, 101, 121], "дубликаты, мусор и уровень 0 отброшены");
        assert!(parse_supply_nodes("").0.is_empty());
        assert!(parse_supply_nodes("\u{feff}1 5").0.contains(&5), "BOM не мешает");
    }

    #[test]
    fn railways_build_a_bidirectional_chain_graph() {
        let graph = parse_railways("3 4 6521 11444 3312 6282 \n2 3 6282 349 3340\n# x\n1 2 7 7\n9 2 1 2\n1 5 10 11\n1 1 77\n").0;
        assert_eq!(graph[&6521], BTreeSet::from([11444]));
        assert_eq!(graph[&11444], BTreeSet::from([6521, 3312]), "связь в обе стороны");
        assert_eq!(graph[&6282], BTreeSet::from([3312, 349]), "цепочки склеиваются общей точкой");
        assert!(!graph.contains_key(&1), "уровень 9 пропущен");
        assert!(!graph.contains_key(&7), "петля и одиночная точка не создают связей");
        assert_eq!(graph[&10], BTreeSet::from([11]), "K больше числа id обрезается");
        assert!(!graph.contains_key(&77));
        assert!(parse_railways("garbage\n\n").0.is_empty());
    }

    #[test]
    fn missing_files_give_empty_resources_without_panic() {
        let missing = Path::new("Z:\\нет\\такой\\папки");
        assert!(load_supply_nodes(missing).0.is_empty());
        assert!(load_railways(missing).0.is_empty());
    }

    #[test]
    fn real_data_files_are_parsed() {
        let root = Path::new("D:\\SteamLibrary\\steamapps\\common\\Hearts of Iron IV");
        if !root.exists() {
            return;
        }
        let nodes = load_supply_nodes(root);
        let graph = load_railways(root);
        assert!(nodes.0.len() > 500, "узлов: {}", nodes.0.len());
        assert!(graph.0.contains_key(&6521) && graph.0[&6521].contains(&11444), "первая линия railways.txt");
        assert!(graph.0.values().all(|n| !n.is_empty()));
    }
}
