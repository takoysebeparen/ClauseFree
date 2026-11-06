//! Карта провинций: загрузка данных HOI4, парсинг стейтов, выбор провинции курсором.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::PrimaryWindow;
use bevy_egui::{egui, EguiContexts};
use image::RgbaImage;

use crate::camera::{CameraSet, MainCamera};
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

/// Карта провинций в оперативной памяти: пиксели + таблицы lookup.
/// Только чтение после старта, поэтому детерминизм не страдает
/// (HashMap используется исключительно для lookup, без итерации).
#[derive(Resource)]
pub struct ProvinceMap {
    image: RgbaImage,
    by_color: HashMap<[u8; 3], ProvinceInfo>,
    /// id провинции -> местность (для вывода по id).
    terrain_by_id: HashMap<u32, String>,
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
    /// `industrial_complex` — гражданские фабрики стейта.
    pub civilian_factories: u32,
    /// `arms_factory` — военные заводы стейта.
    pub military_factories: u32,
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

        app.insert_resource(centers)
            .insert_resource(province_map)
            .insert_resource(registry)
            .init_resource::<HoveredProvince>()
            .init_resource::<SelectedProvince>()
            .init_resource::<ProvinceAdjacency>()
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

/// Граница государств и береговая линия.
const COUNTRY_BORDER_COLOR: [u8; 3] = [25, 25, 25];

/// Базовые цвета ключевых стран (теги HOI4).
fn known_country_color(tag: &str) -> Option<[u8; 3]> {
    Some(match tag {
        "GER" => [80, 80, 80],
        "SOV" => [160, 40, 40],
        "FRA" => [45, 95, 170],
        "ENG" => [180, 50, 50],
        "USA" => [60, 110, 170],
        "ITA" => [80, 130, 80],
        "JAP" => [220, 220, 210],
        "POL" => [200, 110, 140],
        "CHI" => [190, 170, 60],
        "CAN" => [200, 90, 90],
        "AST" => [70, 140, 150],
        "SPR" => [200, 160, 60],
        "TUR" => [150, 110, 60],
        "ROM" => [200, 180, 80],
        "YUG" => [90, 120, 170],
        "HUN" => [120, 160, 100],
        "CZE" => [110, 140, 200],
        "SWE" => [70, 120, 190],
        "FIN" => [150, 190, 220],
        "POR" => [60, 130, 110],
        "BRA" => [90, 170, 90],
        "ARG" => [130, 180, 220],
        "MEX" => [100, 140, 70],
        "RAJ" => [210, 130, 160],
        "SAF" => [120, 150, 60],
        "PER" => [120, 150, 110],
        _ => return None,
    })
}

/// Детерминированный цвет из тега: FNV-1a (не зависит от версии Rust / случайных ключей).
fn hashed_country_color(tag: &str) -> [u8; 3] {
    let mut h: u32 = 0x811c_9dc5;
    for b in tag.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    // Диапазон 70..=219: ярче воды и не белый
    let ch = |shift: u32| 70 + ((h >> shift) & 0xFF) as u8 % 150;
    [ch(0), ch(8), ch(16)]
}

pub fn country_color(tag: &str) -> [u8; 3] {
    known_country_color(tag).unwrap_or_else(|| hashed_country_color(tag))
}

/// Строит RGBA-буфер политической карты того же размера, что и provinces.bmp.
fn build_political_texture(map: &ProvinceMap, registry: &StateRegistry) -> (Vec<u8>, ProvinceAdjacency) {
    // Индекс страны: 0 = вода/нейтральная зона, 1.. = страны. Цвета — в `fills`.
    let mut fills: Vec<[u8; 3]> = vec![WATER_COLOR];
    let mut tag_to_idx: HashMap<&str, u16> = HashMap::new();
    // Цвет провинции в bmp -> индекс страны-владельца (один раз на провинцию, не на пиксель).
    let mut color_to_owner: HashMap<[u8; 3], u16> = HashMap::with_capacity(map.by_color.len());
    for (rgb, info) in &map.by_color {
        let idx = match registry.state_of(info.id) {
            Some(state) => *tag_to_idx.entry(state.owner.as_str()).or_insert_with(|| {
                fills.push(country_color(&state.owner));
                (fills.len() - 1) as u16
            }),
            None => 0,
        };
        color_to_owner.insert(*rgb, idx);
    }
    // Затемнённый цвет для границ провинций внутри страны (color * 0.75).
    let shaded: Vec<[u8; 3]> = fills
        .iter()
        .map(|c| [(c[0] as u16 * 3 / 4) as u8, (c[1] as u16 * 3 / 4) as u8, (c[2] as u16 * 3 / 4) as u8])
        .collect();

    let w = map.width() as usize;
    let h = map.height() as usize;
    let raw = map.image.as_raw();

    // Проход 1: плоский массив владельцев (по 2 байта на пиксель).
    let mut owner = vec![0u16; w * h];
    let mut last_key = [0u8; 3];
    let mut last_val = 0u16;
    let mut have_last = false;
    for (src, o) in raw.chunks_exact(4).zip(owner.iter_mut()) {
        let key = [src[0], src[1], src[2]];
        if !have_last || key != last_key {
            last_val = color_to_owner.get(&key).copied().unwrap_or(0);
            last_key = key;
            have_last = true;
        }
        *o = last_val;
    }

    // Проход 2: заливка + границы. Соседи читаются напрямую из плоских буферов.
    let mut out = vec![255u8; raw.len()];
    // Уникальные рёбра (меньший id, больший id); порядок вставки не влияет на результат.
    let mut edges: HashSet<(u32, u32)> = HashSet::new();
    let mut last_pair = ([0u8; 3], [0u8; 3]); // кэш: подряд идущие одинаковые стыки
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;

            // Смежность: только горизонтальные/вертикальные стыки (правый и нижний сосед
            // покрывают все пары без дублей; диагонали не учитываются).
            let key_here = [raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2]];
            for n in [(x + 1 < w).then(|| i + 1), (y + 1 < h).then(|| i + w)].into_iter().flatten() {
                let key_n = [raw[n * 4], raw[n * 4 + 1], raw[n * 4 + 2]];
                if key_n == key_here || (key_here, key_n) == last_pair {
                    continue;
                }
                last_pair = (key_here, key_n);
                if let (Some(a), Some(b)) = (map.by_color.get(&key_here), map.by_color.get(&key_n)) {
                    if a.id != b.id {
                        edges.insert((a.id.min(b.id), a.id.max(b.id)));
                    }
                }
            }

            let own = owner[i];
            let dst = &mut out[i * 4..i * 4 + 3];

            if own == 0 {
                dst.copy_from_slice(&WATER_COLOR); // в море границы не рисуем
                continue;
            }

            // Соседи: слева, справа, сверху, снизу (за краем карты соседей нет).
            let mut neighbors = [usize::MAX; 4];
            if x > 0 {
                neighbors[0] = i - 1;
            }
            if x + 1 < w {
                neighbors[1] = i + 1;
            }
            if y > 0 {
                neighbors[2] = i - w;
            }
            if y + 1 < h {
                neighbors[3] = i + w;
            }

            let key = &raw[i * 4..i * 4 + 3];
            let mut country_border = false;
            let mut province_border = false;
            for &n in &neighbors {
                if n == usize::MAX {
                    continue;
                }
                if owner[n] != own {
                    country_border = true;
                    break; // высший приоритет, дальше смотреть незачем
                }
                if &raw[n * 4..n * 4 + 3] != key {
                    province_border = true;
                }
            }

            let color = if country_border {
                COUNTRY_BORDER_COLOR
            } else if province_border {
                shaded[own as usize]
            } else {
                fills[own as usize]
            };
            dst.copy_from_slice(&color);
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
    (out, adjacency)
}

pub fn setup_map(
    mut commands: Commands,
    map: Res<ProvinceMap>,
    registry: Res<StateRegistry>,
    mut images: ResMut<Assets<Image>>,
) {
    let (political, adjacency) = build_political_texture(&map, &registry);
    commands.insert_resource(adjacency);
    println!("[УСПЕХ] Политическая карта сгенерирована: {}x{}", map.width(), map.height());

    // Оригинальный provinces.bmp остаётся в RAM (ProvinceMap) для определения провинции под курсором.
    let bevy_image = Image::new(
        Extent3d {
            width: map.width(),
            height: map.height(),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        political,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );

    // Спрайт по центру (0,0), anchor по умолчанию Center, масштаб 1:1.
    commands.spawn(SpriteBundle {
        texture: images.add(bevy_image),
        ..default()
    });
}

// ---------------------------------------------------------------------------
// Парсер стейтов
// ---------------------------------------------------------------------------

/// Разбивает текст Paradox-script на токены: слова, `{`, `}`, `=`, строки в кавычках.
/// Комментарии (`# ...`) отбрасываются. Кавычки с значения снимаются.
fn tokenize(text: &str) -> Vec<String> {
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

fn load_province_map(root: &Path) -> Result<ProvinceMap, String> {
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
    Ok(ProvinceMap { image, by_color, terrain_by_id })
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
    if contexts.ctx_mut().wants_pointer_input() {
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
) {
    let Some(id) = selected.id else { return };
    let ctx = contexts.ctx_mut();

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
                    ui.horizontal(|ui| {
                        ui.label("Контролирует:");
                        let [r, g, b] = country_color(&s.owner);
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 3.0, egui::Color32::from_rgb(r, g, b));
                        ui.label(&s.owner);
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
